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
/// 引擎闲置约 10 分钟会被服务端自动卸载（省内存），这里顺手预热，
/// 让界面显示「已加载」而不是让用户把正常的懒加载当成故障。
#[tauri::command]
pub fn local_asr_health(endpoint: Option<String>) -> Result<Health, String> {
    let base = endpoint.unwrap_or_else(|| DEFAULT_ENDPOINT.to_string());
    let health = ensure_ready(&base)?;
    if health.loaded {
        return Ok(health);
    }
    /* 预热必须用用户实际选择的引擎：固定用 DEFAULT_ENGINE 会把
     * qwen3 选择"证明"成 sensevoice，设置页于是永远显示错的引擎。 */
    let (_, engine) = saved_prefs();
    ensure_loaded(&base, &engine)
}

/// 读取已保存的识别偏好（endpoint + engine）。文件缺失或字段为空时
/// 回落到默认值——与 local_provider_prefs 的语义保持一致。
pub fn saved_prefs() -> (String, String) {
    let home = match std::env::var("HOME") {
        Ok(home) => home,
        Err(_) => return (DEFAULT_ENDPOINT.to_string(), DEFAULT_ENGINE.to_string()),
    };
    let path = std::path::Path::new(&home).join(".hachimo-local-asr.json");
    let Ok(raw) = std::fs::read_to_string(path) else {
        return (DEFAULT_ENDPOINT.to_string(), DEFAULT_ENGINE.to_string());
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return (DEFAULT_ENDPOINT.to_string(), DEFAULT_ENGINE.to_string());
    };
    let endpoint = value
        .get("endpoint")
        .and_then(|item| item.as_str())
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .unwrap_or(DEFAULT_ENDPOINT)
        .to_string();
    let engine = value
        .get("engine")
        .and_then(|item| item.as_str())
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .unwrap_or(DEFAULT_ENGINE)
        .to_string();
    (endpoint, engine)
}

/// 设置页保存语音识别来源。写 `~/.hachimo-local-asr.json`，识别器每次开麦
/// 前读一次，因此切换后下一句就生效，不需要重启客户端。
///
/// 必须读-改-写：这份文件同时存 TTS 偏好（ttsProvider/localTtsVoice）。
/// 之前整文件覆写，保存 ASR 会把 TTS 设置抹掉（实测发生过）。
#[tauri::command]
pub fn local_asr_save_prefs(
    provider: String,
    endpoint: Option<String>,
    engine: Option<String>,
) -> Result<(), String> {
    let home = std::env::var("HOME").map_err(|_| "无法定位 HOME 目录".to_string())?;
    let path = std::path::Path::new(&home).join(".hachimo-local-asr.json");
    let mut value: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if let Some(map) = value.as_object_mut() {
        map.insert("provider".into(), serde_json::json!(provider));
        map.insert(
            "endpoint".into(),
            serde_json::json!(endpoint.unwrap_or_else(|| DEFAULT_ENDPOINT.to_string())),
        );
        map.insert(
            "engine".into(),
            serde_json::json!(engine.unwrap_or_else(|| DEFAULT_ENGINE.to_string())),
        );
    }
    std::fs::write(&path, value.to_string())
        .map_err(|e| format!("保存语音识别偏好失败：{e}"))
}

/* 本地 ASR 服务的部署位置。放在 ~/.hachimo 下，避免 macOS TCC 对
 * "文稿" 目录的授权拦截（脚本与模型都在 Documents 下会导致 spawn 失败）。 */
const SERVICE_PYTHON: &str = "/opt/homebrew/opt/python@3.14/bin/python3.14";
const SERVICE_DIR: &str = ".hachimo";
const SERVICE_SCRIPT_NAME: &str = "stt_server.py";
const SERVICE_MODEL_NAME: &str = "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17";

fn home_dir() -> Result<std::path::PathBuf, String> {
    std::env::var("HOME")
        .map(std::path::PathBuf::from)
        .map_err(|_| "无法定位 HOME 目录".to_string())
}

fn service_script() -> Result<std::path::PathBuf, String> {
    Ok(home_dir()?.join(SERVICE_DIR).join(SERVICE_SCRIPT_NAME))
}

fn service_model_dir() -> Result<std::path::PathBuf, String> {
    Ok(home_dir()?.join(SERVICE_DIR).join("models").join(SERVICE_MODEL_NAME))
}

/// 确保本地 ASR 服务在跑：已经在跑就复用，否则自己拉起来（不占终端、
/// 不依赖 launchd）。返回 true 表示本次是新启动的。
pub fn ensure_service() -> Result<bool, String> {
    if health(DEFAULT_ENDPOINT, Duration::from_millis(900)).is_ok() {
        return Ok(false);
    }
    let script = service_script()?;
    let model_dir = service_model_dir()?;
    std::process::Command::new(SERVICE_PYTHON)
        .arg(&script)
        .env("VIBEBUDDY_STT_PORT", "8900")
        .env("VIBEBUDDY_SENSEVOICE_DIR", &model_dir)
        .env("VIBEBUDDY_STT_LOG", "/tmp/hachimo-stt.log")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("启动本地 ASR 服务失败：{e}（检查 {SERVICE_PYTHON} / {}）", script.display()))?;
    Ok(true)
}

/// 探活并自愈：服务没起就拉起来，最多等 20 秒（SenseVoice 冷加载约 0.5s，
/// 这里留足 Python 启动 + 模型映射的时间）。
pub fn ensure_ready(endpoint: &str) -> Result<Health, String> {
    if let Ok(health) = health(endpoint, Duration::from_millis(1200)) {
        return Ok(health);
    }
    ensure_service()?;
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut last = String::from("本地 ASR 服务无响应");
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(400));
        match health(endpoint, Duration::from_millis(1200)) {
            Ok(health) => return Ok(health),
            Err(error) => last = error,
        }
    }
    Err(last)
}

/// 把引擎拉进内存：服务端是懒加载 + 闲置卸载，first use 前主动发一段
/// 0.5s 静音把加载成本付掉（SenseVoice 约 0.7s）。服务端冷加载失败时
/// 这里会带回真实错误，而不是留给下一次识别报模糊的失败。
pub fn ensure_loaded(endpoint: &str, engine: &str) -> Result<Health, String> {
    let engine = if engine.trim().is_empty() { DEFAULT_ENGINE } else { engine.trim() };
    let health = ensure_ready(endpoint)?;
    if health.loaded && health.engine == engine {
        return Ok(health);
    }
    let silence = pcm_to_wav(&vec![0u8; 8000], 16000, 1); // 0.5s @16k
    let _ = transcribe_wav(endpoint, engine, &silence, Duration::from_secs(30));
    ensure_ready(endpoint)
}
