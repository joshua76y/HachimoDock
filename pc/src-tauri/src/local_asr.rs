/*
 * [Input] 本地 ASR 服务（sherpa-onnx SenseVoice / Qwen3-ASR）的 HTTP 契约。
 * [Output] `Health` / `transcribe_wav` / `pcm_to_wav`：把一段 WAV 交给
 *          http://127.0.0.1:8900/stt 换回文字，并把裸 PCM 包成 RIFF/WAVE。
 * [Pos] PC 端 ASR 的第二种 provider，与 volcengine_asr 并列，无 Key、纯本地。
 * [Sync] 契约来源 RLCD/docs/asr-porting.md §7；改这里要同步该文档与
 *        `pc/docs/realtime-chat.md`。
 *
 * 服务只收 WAV：body 是裸字节（不是 multipart、不是 base64），返回
 *   {"ok":true,"text":"…","engine":"sensevoice","ms":113}
 * 失败返回 {"ok":false,"error":"…"}。引擎可用 ?engine=sensevoice|qwen3 覆盖。
 */

use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// 服务默认监听端口；模型常驻，实测 SenseVoice 单次约 0.1~0.3s。
pub const DEFAULT_ENDPOINT: &str = "http://127.0.0.1:8900";
/// 默认引擎：SenseVoice 快（0.2s / 900MB），Qwen3 准但慢（2s / 5GB）。
pub const DEFAULT_ENGINE: &str = "sensevoice";

#[derive(Debug, Clone, Serialize)]
pub struct Health {
    pub ok: bool,
    pub engine: String,
    pub loaded: bool,
    pub load_sec: f64,
    pub latency_ms: u64,
}

#[derive(Debug, Deserialize)]
struct HealthWire {
    #[serde(default)]
    ok: bool,
    #[serde(default, rename = "engineLoaded")]
    engine_loaded: String,
    #[serde(default)]
    engine: String,
    #[serde(default)]
    loaded: bool,
    #[serde(default, rename = "loadSec")]
    load_sec: f64,
}

#[derive(Debug, Deserialize)]
struct SttWire {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    text: String,
    #[serde(default)]
    error: String,
    #[serde(default)]
    engine: String,
    #[serde(default)]
    ms: u64,
}

/// 识别结果（保留引擎与耗时，界面可以直接显示）。
#[derive(Debug, Clone, Serialize)]
pub struct Transcript {
    pub text: String,
    pub engine: String,
    pub ms: u64,
}

fn join(base: &str, path: &str) -> String {
    format!("{}{}", base.trim_end_matches('/'), path)
}

fn client(timeout: Duration) -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| format!("本地 ASR 客户端创建失败：{e}"))
}

/// `GET /health` —— 设置页保存时探活用；`loaded=false` 表示模型还没进内存。
pub fn health(base: &str, timeout: Duration) -> Result<Health, String> {
    let url = join(base, "/health");
    let started = Instant::now();
    let body = client(timeout)?
        .get(&url)
        .send()
        .map_err(|e| format!("连接本地 ASR 失败（{url}）：{e}"))?
        .text()
        .map_err(|e| format!("读取本地 ASR 响应失败：{e}"))?;
    let wire: HealthWire = serde_json::from_str(&body)
        .map_err(|e| format!("本地 ASR 响应不是合法 JSON：{e}"))?;
    let engine = if !wire.engine_loaded.is_empty() {
        wire.engine_loaded
    } else {
        wire.engine
    };
    Ok(Health {
        ok: wire.ok,
        engine,
        loaded: wire.loaded,
        load_sec: wire.load_sec,
        latency_ms: started.elapsed().as_millis() as u64,
    })
}

/// `POST /stt?engine=…` —— 传整段 WAV（16k/24k 单声道，非 16k 由服务转码）。
pub fn transcribe_wav(
    base: &str,
    engine: &str,
    wav: &[u8],
    timeout: Duration,
) -> Result<Transcript, String> {
    if wav.len() <= 44 {
        return Err("录音太短，未发送识别".into());
    }
    let engine = if engine.trim().is_empty() {
        DEFAULT_ENGINE
    } else {
        engine.trim()
    };
    let url = format!("{}?engine={}", join(base, "/stt"), engine);
    let started = Instant::now();
    let body = client(timeout)?
        .post(&url)
        .header(reqwest::header::CONTENT_TYPE, "audio/wav")
        .body(wav.to_vec())
        .send()
        .map_err(|e| format!("本地 ASR 请求失败（{url}）：{e}"))?
        .text()
        .map_err(|e| format!("读取本地 ASR 响应失败：{e}"))?;
    let wire: SttWire = serde_json::from_str(&body)
        .map_err(|e| format!("本地 ASR 响应不是合法 JSON：{e}"))?;
    if !wire.ok {
        return Err(if wire.error.is_empty() {
            "本地 ASR 返回失败但没有错误信息".into()
        } else {
            wire.error
        });
    }
    Ok(Transcript {
        text: wire.text.trim().to_string(),
        engine: if wire.engine.is_empty() { engine.to_string() } else { wire.engine },
        ms: if wire.ms == 0 {
            started.elapsed().as_millis() as u64
        } else {
            wire.ms
        },
    })
}

/// 裸 PCM → 44 字节 RIFF/WAVE 头 + 数据。设备侧发的是 16k/1ch/s16le，
/// 服务原生支持 16k，不需要 ffmpeg 转码。
pub fn pcm_to_wav(pcm: &[u8], sample_rate: u32, channels: u16) -> Vec<u8> {
    let bits: u16 = 16;
    let block_align = channels * bits / 8;
    let byte_rate = sample_rate * block_align as u32;
    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&((36 + pcm.len()) as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(pcm);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_matches_riff_layout() {
        let pcm = vec![0u8; 3200]; // 100ms @16k mono s16le
        let wav = pcm_to_wav(&pcm, 16000, 1);
        assert_eq!(wav.len(), 44 + pcm.len());
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(u32::from_le_bytes([wav[4], wav[5], wav[6], wav[7]]) as usize, 36 + pcm.len());
        assert_eq!(u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]), 16000);
        assert_eq!(u16::from_le_bytes([wav[22], wav[23]]), 1);
        assert_eq!(u32::from_le_bytes([wav[40], wav[41], wav[42], wav[43]]) as usize, pcm.len());
    }

    #[test]
    fn empty_audio_is_rejected_before_network() {
        let err = transcribe_wav(DEFAULT_ENDPOINT, DEFAULT_ENGINE, &[0u8; 8], Duration::from_millis(50));
        assert!(err.is_err());
    }

    #[test]
    fn engine_falls_back_to_default_when_blank() {
        // 空白引擎应回落到 sensevoice，不发请求（音频过短先被拦下）
        let err = transcribe_wav(DEFAULT_ENDPOINT, "  ", &[], Duration::from_millis(50));
        assert!(err.is_err());
    }
}

/// 设置页「测试连接」用：探测本地服务是否在跑、模型是否已加载。
#[tauri::command]
pub fn local_asr_health(endpoint: Option<String>) -> Result<Health, String> {
    let base = endpoint.unwrap_or_else(|| DEFAULT_ENDPOINT.to_string());
    health(&base, Duration::from_millis(2500))
}

/// 设置页保存语音识别来源。写 `~/.hachimo-local-asr.json`，识别器每次开麦
/// 前读一次，因此切换后下一句就生效，不需要重启客户端。
#[tauri::command]
pub fn local_asr_save_prefs(
    provider: String,
    endpoint: Option<String>,
    engine: Option<String>,
) -> Result<(), String> {
    let home = std::env::var("HOME").map_err(|_| "无法定位 HOME 目录".to_string())?;
    let path = std::path::Path::new(&home).join(".hachimo-local-asr.json");
    let payload = serde_json::json!({
        "provider": provider,
        "endpoint": endpoint.unwrap_or_else(|| DEFAULT_ENDPOINT.to_string()),
        "engine": engine.unwrap_or_else(|| DEFAULT_ENGINE.to_string()),
    });
    std::fs::write(&path, payload.to_string())
        .map_err(|e| format!("保存语音识别偏好失败：{e}"))
}
