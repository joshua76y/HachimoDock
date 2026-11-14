/*
 * [Input] 设备经 Wi-Fi 推来的 HPA1 分帧 PCM（TCP 8898，16k/1ch/s16le）。
 * [Output] VoiceWifiReceiver：校验结束标记、长度和校验值后交给回调
 *          （回调里再拼 WAV 头喂 ASR）；同时暴露统计给界面/日志。
 * [Pos] USB JSONL 之外的第二条音频来源，识别器输入格式与 USB 完全一致。
 * [Sync] 参数与 RLCD/docs/asr-porting.md §6.1 对齐；固件 pet_p4_voice_wifi.c 对端。
 */

use std::io::{self, Read};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// 设备推流的固定端口（与固件 `VOICE_WIFI_PORT` 一致）。
pub const VOICE_WIFI_PORT: u16 = 8898;
/// 设备侧采样率：P4 直接用 16k，不需要转码。
pub const VOICE_WIFI_RATE: u32 = 16_000;
/// 少于 200ms 的录音当误触丢弃。
pub const MIN_PCM_BYTES: usize = (VOICE_WIFI_RATE as usize / 1000) * 2 * 200;
const MAX_PCM_BYTES: usize = 16 * 1024 * 1024;
const MAX_FRAME_BYTES: usize = 4096;

fn read_framed_pcm<R: Read>(reader: &mut R) -> io::Result<Vec<u8>> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid Wi-Fi PCM frame");
    let mut magic = [0u8; 4];
    reader.read_exact(&mut magic)?;
    if &magic != b"HPA1" { return Err(invalid()); }
    let mut buffer = Vec::new();
    let mut checksum = 0x811c9dc5u32;
    loop {
        let mut len = [0u8; 2];
        reader.read_exact(&mut len)?;
        let len = u16::from_le_bytes(len) as usize;
        if len == 0 {
            let mut trailer = [0u8; 8];
            reader.read_exact(&mut trailer)?;
            let bytes = u32::from_le_bytes(trailer[..4].try_into().unwrap()) as usize;
            let expected = u32::from_le_bytes(trailer[4..].try_into().unwrap());
            return if bytes == buffer.len() && expected == checksum { Ok(buffer) } else { Err(invalid()) };
        }
        if len > MAX_FRAME_BYTES || len % 2 != 0 || buffer.len() + len > MAX_PCM_BYTES {
            return Err(invalid());
        }
        let start = buffer.len();
        buffer.resize(start + len, 0);
        reader.read_exact(&mut buffer[start..])?;
        for byte in &buffer[start..] {
            checksum = (checksum ^ u32::from(*byte)).wrapping_mul(0x01000193);
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct VoiceWifiStats {
    pub sessions: u64,
    pub accepted_bytes: u64,
    pub dropped_sessions: u64,
}

/// 累积一帧 PCM。抽成纯函数便于单测（无网络也能验证长度判断）。
pub fn accumulate(buffer: &mut Vec<u8>, chunk: &[u8]) {
    buffer.extend_from_slice(chunk);
}

/// 一次连接是否够长到值得识别。
pub fn is_usable(buffer: &[u8]) -> bool {
    buffer.len() >= MIN_PCM_BYTES
}

pub struct VoiceWifiReceiver {
    running: Arc<AtomicBool>,
    sessions: Arc<AtomicU64>,
    bytes: Arc<AtomicU64>,
    dropped: Arc<AtomicU64>,
    port: u16,
}

impl VoiceWifiReceiver {
    /// 起监听。回调在独立线程里执行，收到的是**一整段** PCM（已按连接累积）。
    pub fn start<F>(port: u16, on_clip: F) -> Result<Self, String>
    where
        F: Fn(Vec<u8>) + Send + Sync + 'static,
    {
        let listener = TcpListener::bind(("0.0.0.0", port))
            .map_err(|e| format!("监听 Wi-Fi 音频端口 {port} 失败：{e}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|e| format!("设置非阻塞失败：{e}"))?;

        let running = Arc::new(AtomicBool::new(true));
        let sessions = Arc::new(AtomicU64::new(0));
        let bytes = Arc::new(AtomicU64::new(0));
        let dropped = Arc::new(AtomicU64::new(0));
        let on_clip = Arc::new(on_clip);

        let (run_flag, session_counter, byte_counter, drop_counter) = (
            running.clone(),
            sessions.clone(),
            bytes.clone(),
            dropped.clone(),
        );

        std::thread::spawn(move || {
            while run_flag.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _addr)) => {
                        let on_clip = on_clip.clone();
                        let session_counter = session_counter.clone();
                        let byte_counter = byte_counter.clone();
                        let drop_counter = drop_counter.clone();
                        std::thread::spawn(move || {
                            session_counter.fetch_add(1, Ordering::SeqCst);
                            handle_connection(stream, on_clip, byte_counter, drop_counter);
                        });
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(40));
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(200)),
                }
            }
        });

        Ok(Self {
            running,
            sessions,
            bytes,
            dropped,
            port,
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn stats(&self) -> VoiceWifiStats {
        VoiceWifiStats {
            sessions: self.sessions.load(Ordering::SeqCst),
            accepted_bytes: self.bytes.load(Ordering::SeqCst),
            dropped_sessions: self.dropped.load(Ordering::SeqCst),
        }
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

impl Drop for VoiceWifiReceiver {
    fn drop(&mut self) {
        self.stop();
    }
}

fn handle_connection<F>(mut stream: TcpStream, on_clip: Arc<F>, bytes: Arc<AtomicU64>, dropped: Arc<AtomicU64>)
where
    F: Fn(Vec<u8>) + Send + Sync + 'static,
{
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    if let Ok(buffer) = read_framed_pcm(&mut stream).and_then(|buffer| {
        if is_usable(&buffer) { Ok(buffer) }
        else { Err(io::Error::new(io::ErrorKind::InvalidData, "short clip")) }
    }) {
        bytes.fetch_add(buffer.len() as u64, Ordering::SeqCst);
        on_clip(buffer);
    } else {
        dropped.fetch_add(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disconnected_incomplete_clip_is_not_delivered() {
        use std::io::Write;
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let dropped = Arc::new(AtomicU64::new(0));
        let dropped_worker = dropped.clone();
        let worker = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            handle_connection(stream, Arc::new(move |data| { let _ = tx.send(data); }),
                              Arc::new(AtomicU64::new(0)), dropped_worker);
        });
        let mut stream = TcpStream::connect(address).unwrap();
        stream.write_all(b"HPA1").unwrap();
        stream.write_all(&vec![0x55; MIN_PCM_BYTES]).unwrap();
        drop(stream);
        worker.join().unwrap();
        assert!(rx.try_recv().is_err());
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn framed_pcm_checks_length_checksum_and_truncation() {
        let pcm = [1u8, 2, 3, 4];
        let mut hash = 0x811c9dc5u32;
        for byte in pcm { hash = (hash ^ u32::from(byte)).wrapping_mul(0x01000193); }
        let mut wire = b"HPA1".to_vec();
        wire.extend_from_slice(&4u16.to_le_bytes());
        wire.extend_from_slice(&pcm);
        wire.extend_from_slice(&0u16.to_le_bytes());
        wire.extend_from_slice(&4u32.to_le_bytes());
        wire.extend_from_slice(&hash.to_le_bytes());
        assert_eq!(read_framed_pcm(&mut &wire[..]).unwrap(), pcm);
        assert!(read_framed_pcm(&mut &wire[..wire.len()-1]).is_err());
        *wire.last_mut().unwrap() ^= 1;
        assert!(read_framed_pcm(&mut &wire[..]).is_err());
        assert!(read_framed_pcm(&mut &b"HPA1\x01\x00x"[..]).is_err());
    }

    #[test]
    fn short_clip_is_dropped() {
        let buffer = vec![0u8; MIN_PCM_BYTES - 1];
        assert!(!is_usable(&buffer));
    }

    #[test]
    fn two_hundred_ms_is_accepted() {
        let buffer = vec![0u8; MIN_PCM_BYTES];
        assert!(is_usable(&buffer));
    }

    #[test]
    fn accumulate_keeps_order_and_length() {
        let mut buffer = Vec::new();
        accumulate(&mut buffer, &[1, 2, 3]);
        accumulate(&mut buffer, &[4, 5]);
        assert_eq!(buffer, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn wifi_pcm_converts_to_wav_for_asr() {
        let pcm = vec![0u8; MIN_PCM_BYTES];
        let wav = crate::local_asr::pcm_to_wav(&pcm, VOICE_WIFI_RATE, 1);
        assert_eq!(wav.len(), 44 + pcm.len());
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]), VOICE_WIFI_RATE);
    }
}
