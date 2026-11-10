/*
 * [Input] 一段中文文本 + 音色/语速。
 * [Output] 设备可播的 16k/1ch/s16le PCM（RIFF/WAVE 封装）。
 * [Pos] 本地 TTS：用 macOS 自带 `say` 合成，再 afconvert 转成设备格式，
 *       完全不依赖云端凭据，与火山 TTS 并列可选。
 * [Sync] 采样率/声道必须与 pet_p4_audio 的播放格式一致（16k/单声道/16bit）。
 */

use std::io::Read;
use std::process::Command;

pub const DEFAULT_VOICE: &str = "Tingting";
pub const SAMPLE_RATE: u32 = 16_000;

#[derive(Debug, Clone, serde::Serialize)]
pub struct TtsResult {
    pub wav: Vec<u8>,
    pub voice: String,
    pub ms: u64,
}

/// 用 macOS `say` 合成语音，再用 `afconvert` 转成 16k 单声道 16bit WAV。
/// 返回的字节可直接作为 WAV 交给播放层。
pub fn synthesize(text: &str, voice: Option<&str>, rate_wpm: Option<u32>) -> Result<TtsResult, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("文本为空，未合成".into());
    }
    let voice = voice.unwrap_or(DEFAULT_VOICE);
    let started = std::time::Instant::now();
    /* 临时名必须唯一：两个测试线程并发合成时 pid 相同，同名文件会让 say 报 -54。 */
    let stamp = stamp();
    let aiff = std::env::temp_dir().join(format!("hachimo-tts-{stamp}.aiff"));
    let wav = std::env::temp_dir().join(format!("hachimo-tts-{stamp}.wav"));

    let mut say = Command::new("/usr/bin/say");
    say.arg("-v").arg(voice).arg("-o").arg(&aiff);
    if let Some(rate) = rate_wpm {
        // say 用每分钟词数控制语速，中文常用 180~260
        say.arg("-r").arg(rate.clamp(80, 400).to_string());
    }
    say.arg(text);
    let say_out = say.output().map_err(|e| format!("调用 say 失败：{e}"))?;
    if !say_out.status.success() {
        return Err(format!(
            "say 合成失败：{}",
            String::from_utf8_lossy(&say_out.stderr).trim()
        ));
    }

    let convert = Command::new("/usr/bin/afconvert")
        .args(["-f", "WAVE", "-d", "LEI16@16000", "-c", "1"])
        .arg(&aiff)
        .arg(&wav)
        .output()
        .map_err(|e| format!("调用 afconvert 失败：{e}"))?;
    if !convert.status.success() {
        let _ = std::fs::remove_file(&aiff);
        return Err(format!(
            "afconvert 转码失败：{}",
            String::from_utf8_lossy(&convert.stderr).trim()
        ));
    }

    let mut bytes = Vec::new();
    std::fs::File::open(&wav)
        .and_then(|mut file| file.read_to_end(&mut bytes))
        .map_err(|e| format!("读取合成结果失败：{e}"))?;
    let _ = std::fs::remove_file(&aiff);
    let _ = std::fs::remove_file(&wav);

    if bytes.len() <= 44 {
        return Err("合成结果为空".into());
    }
    /* 模型未下载/读不了中文的音色：say exit 0 但只有 ~4.6KB 空壳，
     * 转出的 WAV 近乎纯静音。给个明确错误，别让设备播一段无声。 */
    if bytes.len() < 4_000 {
        return Err(format!("音色 {voice} 无法合成这段文本（输出近乎为空，可能未下载该语音模型）"));
    }
    Ok(TtsResult {
        wav: bytes,
        voice: voice.to_string(),
        ms: started.elapsed().as_millis() as u64,
    })
}

/// 列出系统全部音色名（不做可用性过滤，测试用）。
pub fn available_voices() -> Vec<String> {
    voice_inventory().into_iter().map(|(name, _)| name).collect()
}

/// `say -v '?'` 的原始清单：音色名 + locale。名字可能含空格（如 "Bad News"）。
fn voice_inventory() -> Vec<(String, String)> {
    let output = Command::new("/usr/bin/say").arg("-v").arg("?").output();
    let Ok(output) = output else { return vec![] };
    let text = String::from_utf8_lossy(&output.stdout);
    let mut voices: Vec<(String, String)> = text
        .lines()
        .filter_map(|line| {
            let head = line.split('#').next()?;
            let mut parts = head.split_whitespace();
            let locale = parts.next_back()?.to_string();
            let name = parts.map(|part| format!("{part} ")).collect::<String>();
            let name = name.trim_end().to_string();
            if name.is_empty() || locale.is_empty() { None } else { Some((name, locale)) }
        })
        .collect();
    voices.sort();
    voices.dedup();
    voices
}

/// 探测一个音色能否真正发声：用中文短语合成，AIFF 输出超过阈值才算可用。
/// 「模型未下载/读不了中文」的音色 exit code 仍是 0，但输出只有 ~4.6KB 空壳。
const VOICE_PROBE_TEXT: &str = "你好，今天天气怎么样";
const VOICE_PROBE_MIN_BYTES: u64 = 10_000;

fn probe_voice_speakable(name: &str) -> bool {
    let path = std::env::temp_dir().join(format!("hachimo-voice-probe-{}.aiff", stamp()));
    let Ok(_) = Command::new("/usr/bin/say")
        .arg("-v").arg(name)
        .arg("-o").arg(&path)
        .arg(VOICE_PROBE_TEXT)
        .output()
    else {
        return false;
    };
    let size = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
    let _ = std::fs::remove_file(&path);
    size >= VOICE_PROBE_MIN_BYTES
}

/// 并发探测时保证临时文件名唯一：pid + 原子序号。
fn stamp() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    format!("{}-{}", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct VoiceAvailability {
    pub name: String,
    pub available: bool,
}

/// 探测缓存的磁盘位置与格式：键是 `say -v '?'` 原文的哈希，
/// 系统音色列表变化（下载/删除音色）时自动失效重测。
fn voice_cache_path() -> Result<std::path::PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| "无法定位 HOME 目录".to_string())?;
    Ok(std::path::Path::new(&home)
        .join(".hachimo")
        .join("tts-voices-cache.json"))
}

fn voice_cache_load(inventory_hash: &str) -> Option<Vec<VoiceAvailability>> {
    let raw = std::fs::read_to_string(voice_cache_path().ok()?).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    if value.get("inventoryHash").and_then(|item| item.as_str()) != Some(inventory_hash) {
        return None;
    }
    let entries = value.get("voices")?.as_array()?;
    Some(entries
        .iter()
        .filter_map(|item| Some(VoiceAvailability {
            name: item.get("name")?.as_str()?.to_string(),
            available: item.get("available")?.as_bool()?,
        }))
        .collect())
}

fn voice_cache_save(inventory_hash: &str, voices: &[VoiceAvailability]) {
    let Ok(path) = voice_cache_path() else { return };
    let value = serde_json::json!({
        "inventoryHash": inventory_hash,
        "savedAt": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        "voices": voices,
    });
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, value.to_string());
}

/// 逐音色探测可用性。188 个音色串行要 ~110s，12 路并发压到 ~10s；
/// 结果落盘缓存，之后设置页秒开。
fn probe_voices_parallel(inventory: &[(String, String)]) -> Vec<VoiceAvailability> {
    const WORKERS: usize = 12;
    let names: Vec<String> = inventory.iter().map(|(name, _)| name.clone()).collect();
    let per_worked = ((names.len() + WORKERS - 1) / WORKERS).max(1);
    let chunks: Vec<Vec<String>> = names
        .chunks(per_worked)
        .map(<[String]>::to_vec)
        .collect();
    let mut voices: Vec<VoiceAvailability> = std::thread::scope(|scope| {
        let handles: Vec<_> = chunks
            .iter()
            .map(|chunk| {
                scope.spawn(move || {
                    chunk
                        .iter()
                        .map(|name| VoiceAvailability {
                            name: name.clone(),
                            available: probe_voice_speakable(name),
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().unwrap_or_default())
            .collect()
    });
    voices.sort_by(|a, b| a.name.cmp(&b.name));
    voices
}

/// 设置页下拉：只保留能真正发声的音色（探测结果带缓存）。
/// 前端在全部音色里挑，`available=false` 的直接不显示。
/// 必须异步：首次探测要跑十几秒 `say` 子进程，同步命令会阻塞
/// Tauri 主线程把整个 UI 冻住（表现为"切换页面卡很久"）。
#[tauri::command]
pub async fn local_tts_voices() -> Result<Vec<VoiceAvailability>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let inventory = voice_inventory();
        if inventory.is_empty() {
            return vec![VoiceAvailability { name: DEFAULT_VOICE.to_string(), available: true }];
        }
        let hash = md5_hex(&format!("{inventory:?}"));
        if let Some(cached) = voice_cache_load(&hash) {
            return cached;
        }
        let voices = probe_voices_parallel(&inventory);
        voice_cache_save(&hash, &voices);
        voices
    })
    .await
    .map_err(|e| format!("音色探测任务失败：{e}"))
}

/// 轻量 FNV-1a：只为给音色清单生成缓存键，不追求密码学强度。
fn md5_hex(input: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in input.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

/// 同样异步化：合成要起 `say` 子进程，同步会冻 UI。
#[tauri::command]
pub async fn local_tts_preview(
    text: String,
    voice: Option<String>,
) -> Result<LocalTtsPreview, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let result = synthesize(&text, voice.as_deref(), None)?;
        use base64::Engine as _;
        Ok(LocalTtsPreview {
            bytes: result.wav.len(),
            voice: result.voice,
            ms: result.ms,
            /* 试听按钮要真的能听到声音：把 WAV 带回前端用 <audio> 播。 */
            wav_base64: base64::engine::general_purpose::STANDARD.encode(result.wav),
        })
    })
    .await
    .map_err(|e| format!("TTS 合成任务失败：{e}"))?
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalTtsPreview {
    pub bytes: usize,
    pub voice: String,
    pub ms: u64,
    pub wav_base64: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthesizes_wav_for_chinese_text() {
        let result = synthesize("你好，这是本地语音合成测试", None, None).expect("tts");
        assert!(result.wav.len() > 44, "应当有音频数据");
        assert_eq!(&result.wav[0..4], b"RIFF");
        assert_eq!(&result.wav[8..12], b"WAVE");
        assert_eq!(
            u32::from_le_bytes([result.wav[24], result.wav[25], result.wav[26], result.wav[27]]),
            SAMPLE_RATE
        );
        assert_eq!(u16::from_le_bytes([result.wav[22], result.wav[23]]), 1);
    }

    #[test]
    fn rejects_empty_text() {
        assert!(synthesize("   ", None, None).is_err());
    }

    #[test]
    fn voice_list_is_not_empty() {
        assert!(!available_voices().is_empty());
    }

    #[test]
    fn known_good_voice_probes_available() {
        // 中文音色 Tingting 一定在系统里且能发声；探测缓存不命中时才真正跑 say。
        let inventory = voice_inventory();
        assert!(inventory.iter().any(|(name, _)| name == "Tingting"));
        assert!(probe_voice_speakable("Tingting"));
    }

    #[test]
    fn fnv_hex_is_stable() {
        assert_eq!(md5_hex(""), "cbf29ce484222325"); // FNV-1a 64 空串=偏移基数
        assert_eq!(md5_hex("a").len(), 16);
    }
}

fn prefs_path() -> Result<std::path::PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| "无法定位 HOME 目录".to_string())?;
    Ok(std::path::Path::new(&home).join(".hachimo-local-asr.json"))
}

/// 是否选用本地 TTS（与 ASR 共用一份偏好文件，字段 ttsProvider）。
pub fn provider_is_local() -> bool {
    let Ok(path) = prefs_path() else { return false };
    let Ok(raw) = std::fs::read_to_string(path) else { return false };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else { return false };
    value.get("ttsProvider").and_then(|item| item.as_str()) == Some("local")
}

/// 本地 TTS 使用的系统音色（缺省 Tingting）。
pub fn preferred_voice() -> String {
    let Ok(path) = prefs_path() else { return DEFAULT_VOICE.to_string() };
    let Ok(raw) = std::fs::read_to_string(path) else { return DEFAULT_VOICE.to_string() };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return DEFAULT_VOICE.to_string();
    };
    value
        .get("localTtsVoice")
        .and_then(|item| item.as_str())
        .filter(|item| !item.trim().is_empty())
        .unwrap_or(DEFAULT_VOICE)
        .to_string()
}

/// 设置页保存 TTS 引擎选择（volcano / local）与音色。
#[tauri::command]
pub fn local_tts_save_prefs(provider: String, voice: Option<String>) -> Result<(), String> {
    let path = prefs_path()?;
    let mut value: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if let Some(map) = value.as_object_mut() {
        map.insert("ttsProvider".into(), serde_json::json!(provider));
        if let Some(voice) = voice {
            map.insert("localTtsVoice".into(), serde_json::json!(voice));
        }
    }
    std::fs::write(&path, value.to_string())
        .map_err(|e| format!("保存 TTS 偏好失败：{e}"))
}
