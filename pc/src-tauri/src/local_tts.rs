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
    let stamp = std::process::id();
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
    Ok(TtsResult {
        wav: bytes,
        voice: voice.to_string(),
        ms: started.elapsed().as_millis() as u64,
    })
}

/// 列出系统可用音色，供设置页下拉。
pub fn available_voices() -> Vec<String> {
    let output = Command::new("/usr/bin/say").arg("-v").arg("?").output();
    let Ok(output) = output else { return vec![DEFAULT_VOICE.to_string()] };
    let text = String::from_utf8_lossy(&output.stdout);
    let mut voices: Vec<String> = text
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|name| !name.is_empty())
        .map(|name| name.to_string())
        .collect();
    voices.sort();
    voices.dedup();
    if voices.is_empty() {
        voices.push(DEFAULT_VOICE.to_string());
    }
    voices
}

#[tauri::command]
pub fn local_tts_voices() -> Vec<String> {
    available_voices()
}

#[tauri::command]
pub fn local_tts_preview(text: String, voice: Option<String>) -> Result<LocalTtsPreview, String> {
    let result = synthesize(&text, voice.as_deref(), None)?;
    Ok(LocalTtsPreview {
        bytes: result.wav.len(),
        voice: result.voice,
        ms: result.ms,
    })
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalTtsPreview {
    pub bytes: usize,
    pub voice: String,
    pub ms: u64,
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
