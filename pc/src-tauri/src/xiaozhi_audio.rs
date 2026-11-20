#[cfg(test)]
mod tests {
    use super::*;

    const KEY: [u8; 16] = [0x11; 16];
    const NONCE: [u8; 16] = [0x22; 16];

    #[test]
    fn three_20ms_pcm_frames_become_one_60ms_opus_packet() {
        let mut encoder = PcmToOpus::new().expect("16 kHz mono encoder");
        assert!(encoder.push_pcm_20ms(&vec![0_u8; 640]).unwrap().is_none());
        assert!(encoder.push_pcm_20ms(&vec![1_u8; 640]).unwrap().is_none());
        let opus = encoder
            .push_pcm_20ms(&vec![2_u8; 640])
            .unwrap()
            .expect("third frame completes a 60 ms packet");
        assert!(!opus.is_empty());
        assert!(opus.len() <= u16::MAX as usize);
    }

    #[test]
    fn encrypted_udp_packet_round_trips_and_preserves_network_header() {
        let original = UdpPacket::new(0x1234_5678, 960, 7, vec![1, 2, 3, 4]).unwrap();
        let encrypted = encrypt_udp_payload(&original, &KEY, &NONCE).unwrap();
        assert_eq!(encrypted[0], 0x01);
        assert_eq!(encrypted[1], 0);
        assert_eq!(&encrypted[2..4], &[0, 4]);
        assert_eq!(&encrypted[4..8], &0x1234_5678_u32.to_be_bytes());
        assert_eq!(&encrypted[8..12], &960_u32.to_be_bytes());
        assert_eq!(&encrypted[12..16], &7_u32.to_be_bytes());
        assert_eq!(
            decrypt_udp_payload(&encrypted, &KEY, &NONCE, 0).unwrap(),
            original
        );
    }

    #[test]
    fn outgoing_packet_preserves_server_nonce_ssrc() {
        let server_nonce = [
            0x01, 0x00, 0xaa, 0xbb, 0x12, 0x34, 0x56, 0x78,
            0xcc, 0xdd, 0xee, 0xff, 0x11, 0x22, 0x33, 0x44,
        ];
        let packet = UdpPacket::new(0xfeed_beef, 960, 7, vec![1, 2, 3, 4]).unwrap();
        let encrypted = encrypt_outgoing_udp_payload(&packet, &KEY, &server_nonce).unwrap();

        assert_eq!(&encrypted[..2], &[0x01, 0x00]);
        assert_eq!(&encrypted[2..4], &[0, 4]);
        assert_eq!(&encrypted[4..8], &[0x12, 0x34, 0x56, 0x78]);
        assert_eq!(&encrypted[8..12], &960_u32.to_be_bytes());
        assert_eq!(&encrypted[12..16], &7_u32.to_be_bytes());
    }

    #[test]
    fn malformed_or_replayed_udp_packets_are_rejected() {
        let valid = encrypt_udp_payload(
            &UdpPacket::new(1, 960, 7, vec![1, 2, 3, 4]).unwrap(),
            &KEY,
            &NONCE,
        )
        .unwrap();
        let mut wrong_type = valid.clone();
        wrong_type[0] = 2;
        assert!(decrypt_udp_payload(&wrong_type, &KEY, &NONCE, 0).is_err());
        let mut wrong_length = valid.clone();
        wrong_length[2..4].copy_from_slice(&5_u16.to_be_bytes());
        assert!(decrypt_udp_payload(&wrong_length, &KEY, &NONCE, 0).is_err());
        assert!(decrypt_udp_payload(&valid, &KEY, &NONCE, 7).is_err());
    }

    #[test]
    fn downsampled_pcm_is_exactly_16khz_length() {
        let pcm_24k: Vec<i16> = (0..1440).map(|value| value as i16).collect();
        let pcm_16k = resample_to_16k(&pcm_24k, 24_000).unwrap();
        assert_eq!(pcm_16k.len(), 960);
    }
}
use aes::cipher::{KeyIvInit, StreamCipher};
use ctr::Ctr128BE;
use opus::{Application, Channels, Decoder, Encoder};
use std::fmt;

const PCM_20MS_BYTES: usize = 640;
const PCM_60MS_SAMPLES: usize = 960;
const UDP_HEADER_BYTES: usize = 16;
type Aes128Ctr = Ctr128BE<aes::Aes128>;

pub(crate) struct PcmToOpus {
    encoder: Encoder,
    pending: Vec<u8>,
}

impl PcmToOpus {
    pub(crate) fn new() -> Result<Self, XiaozhiAudioError> {
        Ok(Self {
            encoder: Encoder::new(16_000, Channels::Mono, Application::Voip)
                .map_err(|_| XiaozhiAudioError::new("无法创建小智 Opus 编码器"))?,
            pending: Vec::with_capacity(PCM_20MS_BYTES * 3),
        })
    }

    pub(crate) fn push_pcm_20ms(
        &mut self,
        frame: &[u8],
    ) -> Result<Option<Vec<u8>>, XiaozhiAudioError> {
        if frame.len() != PCM_20MS_BYTES {
            return Err(XiaozhiAudioError::new("小智 PCM 帧长度无效"));
        }
        self.pending.extend_from_slice(frame);
        if self.pending.len() < PCM_20MS_BYTES * 3 {
            return Ok(None);
        }
        let samples: Vec<i16> = self
            .pending
            .chunks_exact(2)
            .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        self.pending.clear();
        if samples.len() != PCM_60MS_SAMPLES {
            return Err(XiaozhiAudioError::new("小智 PCM 聚合长度无效"));
        }
        self.encoder
            .encode_vec(&samples, 1276)
            .map(Some)
            .map_err(|_| XiaozhiAudioError::new("小智 Opus 编码失败"))
    }
}

pub(crate) struct OpusToPcm {
    decoder: Decoder,
    sample_rate: u32,
}

impl OpusToPcm {
    pub(crate) fn new(sample_rate: u32) -> Result<Self, XiaozhiAudioError> {
        if !matches!(sample_rate, 16_000 | 24_000) {
            return Err(XiaozhiAudioError::new("小智下行采样率不支持"));
        }
        Ok(Self {
            decoder: Decoder::new(sample_rate, Channels::Mono)
                .map_err(|_| XiaozhiAudioError::new("无法创建小智 Opus 解码器"))?,
            sample_rate,
        })
    }
    pub(crate) fn decode_to_16k(&mut self, opus: &[u8]) -> Result<Vec<i16>, XiaozhiAudioError> {
        let mut decoded = vec![0_i16; self.sample_rate as usize * 60 / 1000];
        let len = self
            .decoder
            .decode(opus, &mut decoded, false)
            .map_err(|_| XiaozhiAudioError::new("小智 Opus 解码失败"))?;
        decoded.truncate(len);
        resample_to_16k(&decoded, self.sample_rate)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct UdpPacket {
    ssrc: u32,
    timestamp: u32,
    sequence: u32,
    payload: Vec<u8>,
}

impl UdpPacket {
    pub(crate) fn new(
        ssrc: u32,
        timestamp: u32,
        sequence: u32,
        payload: Vec<u8>,
    ) -> Result<Self, XiaozhiAudioError> {
        if payload.is_empty() || payload.len() > u16::MAX as usize {
            return Err(XiaozhiAudioError::new("小智 UDP 音频长度无效"));
        }
        Ok(Self {
            ssrc,
            timestamp,
            sequence,
            payload,
        })
    }

    pub(crate) fn sequence(&self) -> u32 { self.sequence }

    pub(crate) fn payload(&self) -> &[u8] { &self.payload }
}

impl fmt::Debug for UdpPacket {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UdpPacket")
            .field("ssrc", &self.ssrc)
            .field("timestamp", &self.timestamp)
            .field("sequence", &self.sequence)
            .field("payload", &"<redacted>")
            .finish()
    }
}

pub(crate) fn encrypt_udp_payload(
    packet: &UdpPacket,
    key: &[u8; 16],
    nonce: &[u8; 16],
) -> Result<Vec<u8>, XiaozhiAudioError> {
    let mut header = udp_header(packet);
    let mut payload = packet.payload.clone();
    let iv = ctr_iv(nonce, &header)?;
    Aes128Ctr::new(key.into(), (&iv).into()).apply_keystream(&mut payload);
    header.extend_from_slice(&payload);
    Ok(header)
}

/// 小智的服务端 hello 会下发一个既是 AES-CTR 初始 nonce、又是 UDP
/// 包头模板的 16 字节字段。上行时只能改动长度、时间戳与序号；尤其不能
/// 重建第 4–8 字节的 SSRC，否则服务端无法把音频归属到已协商的会话。
pub(crate) fn encrypt_outgoing_udp_payload(
    packet: &UdpPacket,
    key: &[u8; 16],
    server_nonce: &[u8; 16],
) -> Result<Vec<u8>, XiaozhiAudioError> {
    let mut header = *server_nonce;
    let payload_len = u16::try_from(packet.payload.len())
        .map_err(|_| XiaozhiAudioError::new("小智 UDP 音频长度无效"))?;
    header[2..4].copy_from_slice(&payload_len.to_be_bytes());
    header[8..12].copy_from_slice(&packet.timestamp.to_be_bytes());
    header[12..16].copy_from_slice(&packet.sequence.to_be_bytes());
    let mut payload = packet.payload.clone();
    Aes128Ctr::new(key.into(), (&header).into()).apply_keystream(&mut payload);
    let mut encrypted = header.to_vec();
    encrypted.extend_from_slice(&payload);
    Ok(encrypted)
}

pub(crate) fn decrypt_udp_payload(
    bytes: &[u8],
    key: &[u8; 16],
    nonce: &[u8; 16],
    last_sequence: u32,
) -> Result<UdpPacket, XiaozhiAudioError> {
    if bytes.len() <= UDP_HEADER_BYTES || bytes[0] != 0x01 || bytes[1] != 0 {
        return Err(XiaozhiAudioError::new("小智 UDP 包头无效"));
    }
    let payload_len = u16::from_be_bytes([bytes[2], bytes[3]]) as usize;
    if bytes.len() != UDP_HEADER_BYTES + payload_len {
        return Err(XiaozhiAudioError::new("小智 UDP 包长度无效"));
    }
    let sequence = u32::from_be_bytes(bytes[12..16].try_into().unwrap());
    if sequence <= last_sequence {
        return Err(XiaozhiAudioError::new("小智 UDP 包序号无效"));
    }
    let iv = ctr_iv(nonce, &bytes[..UDP_HEADER_BYTES])?;
    let mut payload = bytes[UDP_HEADER_BYTES..].to_vec();
    Aes128Ctr::new(key.into(), (&iv).into()).apply_keystream(&mut payload);
    UdpPacket::new(
        u32::from_be_bytes(bytes[4..8].try_into().unwrap()),
        u32::from_be_bytes(bytes[8..12].try_into().unwrap()),
        sequence,
        payload,
    )
}

fn ctr_iv(nonce: &[u8; 16], header: &[u8]) -> Result<[u8; 16], XiaozhiAudioError> {
    if header.len() != UDP_HEADER_BYTES {
        return Err(XiaozhiAudioError::new("小智 UDP 包头无效"));
    }
    let mut iv = *nonce;
    iv[2..4].copy_from_slice(&header[2..4]);
    iv[8..12].copy_from_slice(&header[8..12]);
    iv[12..16].copy_from_slice(&header[12..16]);
    Ok(iv)
}

fn udp_header(packet: &UdpPacket) -> Vec<u8> {
    let mut header = Vec::with_capacity(UDP_HEADER_BYTES);
    header.extend_from_slice(&[0x01, 0]);
    header.extend_from_slice(&(packet.payload.len() as u16).to_be_bytes());
    header.extend_from_slice(&packet.ssrc.to_be_bytes());
    header.extend_from_slice(&packet.timestamp.to_be_bytes());
    header.extend_from_slice(&packet.sequence.to_be_bytes());
    header
}

pub(crate) fn resample_to_16k(
    input: &[i16],
    input_rate: u32,
) -> Result<Vec<i16>, XiaozhiAudioError> {
    match input_rate {
        16_000 => Ok(input.to_vec()),
        24_000 => Ok((0..input.len() * 2 / 3)
            .map(|index| input[index * 3 / 2])
            .collect()),
        _ => Err(XiaozhiAudioError::new("小智下行采样率不支持")),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct XiaozhiAudioError(&'static str);
impl XiaozhiAudioError {
    const fn new(message: &'static str) -> Self {
        Self(message)
    }
}
impl fmt::Display for XiaozhiAudioError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}
impl std::error::Error for XiaozhiAudioError {}
