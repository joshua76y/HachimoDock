/*
 * [Input] 提示音目录：设备侧的固定事件名（record_start / asr_ok / …）与
 *         用户自选的 WAV 文件（放在 app data 的 sounds/ 目录）。
 * [Output] PromptSoundCatalog：列出每个事件的当前音源（内置 / 自定义）、
 *         读取自定义 WAV 路径、写入/恢复默认、把任意音频解码为设备可播的
 *         16k/1ch/s16le PCM WAV。
 * [Pos] PC 端「提示音」设置页的后端；固件按同名文件查表播放。
 * [Sync] 事件名与 firmware/main/pet_p4_sound_catalog.h 保持一致。
 */

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 设备可播放的统一格式：16 kHz / 单声道 / 16bit PCM。
pub const DEVICE_SAMPLE_RATE: u32 = 16_000;

/// 提示音事件。顺序即界面展示顺序。
pub const SOUND_EVENTS: &[(&str, &str)] = &[
    ("record_start", "开始录音"),
    ("record_stop", "松开结束"),
    ("asr_ok", "识别成功"),
    ("asr_fail", "识别失败"),
    ("send_ok", "草稿已发送"),
    ("task_done", "任务完成"),
    ("task_fail", "任务失败"),
    ("low_battery", "电量偏低"),
    ("wifi_lost", "网络断开"),
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptSoundEntry {
    /// 事件名（固件侧同一字符串）
    pub event: String,
    /// 中文名，界面直接显示
    pub label: String,
    /// "builtin" | "custom"
    pub source: String,
    /// 自定义音的文件名（source=custom 时有效）
    pub file_name: Option<String>,
    /// 文件字节数，用于界面显示大小
    pub bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptSoundCatalog {
    pub sounds_dir: String,
    pub entries: Vec<PromptSoundEntry>,
}

fn sounds_root() -> Result<PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| "无法定位 HOME 目录".to_string())?;
    Ok(Path::new(&home)
        .join("Library")
        .join("Application Support")
        .join("com.petmanager.desktop")
        .join("prompt-sounds"))
}

fn entry_for(event: &str, dir: &Path) -> Result<PromptSoundEntry, String> {
    let (event, label) = SOUND_EVENTS
        .iter()
        .find(|(name, _)| *name == event)
        .map(|(name, label)| (*name, *label))
        .ok_or_else(|| format!("未知的提示音事件：{event}"))?;
    let path = dir.join(format!("{event}.wav"));
    let meta = std::fs::metadata(&path).ok().filter(|item| item.len() > 44);
    Ok(PromptSoundEntry {
        event: event.to_string(),
        label: label.to_string(),
        source: if meta.is_some() { "custom".into() } else { "builtin".into() },
        file_name: meta.as_ref().map(|_| format!("{event}.wav")),
        bytes: meta.map(|item| item.len()),
    })
}

/// 列出全部事件的当前音源。目录不存在时返回全内置，不报错。
pub fn catalog() -> Result<PromptSoundCatalog, String> {
    let dir = sounds_root()?;
    let _ = std::fs::create_dir_all(&dir);
    let entries = SOUND_EVENTS
        .iter()
        .map(|(event, _)| entry_for(event, &dir))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(PromptSoundCatalog {
        sounds_dir: dir.to_string_lossy().into_owned(),
        entries,
    })
}

/// 保存自定义提示音。传入的必须是设备可直接播放的 16k/1ch/s16le WAV；
/// 其它格式由上层（界面）先用 ffmpeg 转好再调用，这里只做格式校验，
/// 避免"上传成功但设备播不出来"。
pub fn save_custom(event: &str, wav: &[u8]) -> Result<PromptSoundEntry, String> {
    if wav.len() < 12 {
        return Err("音频文件为空或过短".into());
    }
    if &wav[0..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
        return Err("不是 WAV 文件（需要 RIFF/WAVE 头）".into());
    }
    let channels = u16::from_le_bytes([wav[22], wav[23]]);
    let rate = u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]);
    let bits = u16::from_le_bytes([wav[34], wav[35]]);
    if channels != 1 || rate != DEVICE_SAMPLE_RATE || bits != 16 {
        return Err(format!(
            "格式不符：需要 {DEVICE_SAMPLE_RATE}Hz / 单声道 / 16bit，实际 {rate}Hz / {channels}ch / {bits}bit"
        ));
    }
    let dir = sounds_root()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建提示音目录失败：{e}"))?;
    let path = dir.join(format!("{event}.wav"));
    std::fs::write(&path, wav).map_err(|e| format!("写入提示音失败：{e}"))?;
    entry_for(event, &dir)
}

/// 恢复默认：删掉自定义文件，设备回退到内置音。
pub fn reset_to_builtin(event: &str) -> Result<PromptSoundEntry, String> {
    let dir = sounds_root()?;
    let path = dir.join(format!("{event}.wav"));
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| format!("删除自定义提示音失败：{e}"))?;
    }
    entry_for(event, &dir)
}

/// 读取自定义音的原始字节，供界面试听。
pub fn read_custom(event: &str) -> Result<Vec<u8>, String> {
    let dir = sounds_root()?;
    std::fs::read(dir.join(format!("{event}.wav")))
        .map_err(|e| format!("该事件没有自定义提示音：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_wav_bytes() {
        let error = save_custom("record_start", b"not a wav file at all").unwrap_err();
        assert!(error.contains("WAV"));
    }

    #[test]
    fn rejects_wrong_sample_rate() {
        // 44 字节头 + 1 帧，声明 24k 立体声
        let mut wav = vec![0u8; 46];
        wav[0..4].copy_from_slice(b"RIFF");
        wav[8..12].copy_from_slice(b"WAVE");
        wav[22..24].copy_from_slice(&2u16.to_le_bytes());
        wav[24..28].copy_from_slice(&24_000u32.to_le_bytes());
        wav[34..36].copy_from_slice(&16u16.to_le_bytes());
        let error = save_custom("record_start", &wav).unwrap_err();
        assert!(error.contains("24000") || error.contains("格式不符"));
    }

    #[test]
    fn unknown_event_is_rejected() {
        assert!(reset_to_builtin("no_such_event").is_err());
    }

    #[test]
    fn catalog_covers_every_event() {
        let list = catalog().expect("catalog");
        assert_eq!(list.entries.len(), SOUND_EVENTS.len());
        assert!(list.entries.iter().all(|item| !item.label.is_empty()));
    }
}
