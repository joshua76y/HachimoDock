/*
 * [Input] ASR/TTS 的非秘密 provider 选择与本地端点、模型、音色偏好。
 * [Output] app-data/voice-service-settings.json；所有运行期路由均从此读取。
 * [Pos] 不含云端 Key。云端 Key 继续由 volcengine_asr / voice_chat_settings
 *        的加密配置保存，避免把凭据写入普通 JSON。
 */

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const FILE_NAME: &str = "voice-service-settings.json";
const DEFAULT_ASR_ENDPOINT: &str = "http://127.0.0.1:8900";
const DEFAULT_KOKORO_ENDPOINT: &str = "http://127.0.0.1:8891";

static STORAGE_DIR: OnceLock<PathBuf> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AsrPreferences {
    pub provider: String,
    pub endpoint: String,
    pub engine: String,
}

impl Default for AsrPreferences {
    fn default() -> Self {
        Self { provider: "volcengine".into(), endpoint: DEFAULT_ASR_ENDPOINT.into(), engine: "sensevoice".into() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TtsPreferences {
    pub provider: String,
    pub macos_voice: String,
    pub kokoro_endpoint: String,
    pub kokoro_voice: String,
}

impl Default for TtsPreferences {
    fn default() -> Self {
        Self {
            provider: "volcengine".into(),
            macos_voice: crate::local_tts::DEFAULT_VOICE.into(),
            kokoro_endpoint: DEFAULT_KOKORO_ENDPOINT.into(),
            kokoro_voice: "zf_001".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VoiceServicePreferences {
    pub version: u8,
    pub asr: AsrPreferences,
    pub tts: TtsPreferences,
}

impl Default for VoiceServicePreferences {
    fn default() -> Self { Self { version: 1, asr: AsrPreferences::default(), tts: TtsPreferences::default() } }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceServicePreferencesInput {
    pub asr_provider: Option<String>,
    pub local_asr_endpoint: Option<String>,
    pub local_asr_engine: Option<String>,
    pub tts_provider: Option<String>,
    pub macos_tts_voice: Option<String>,
    pub kokoro_endpoint: Option<String>,
    pub kokoro_voice: Option<String>,
}

pub fn configure_storage_dir(path: PathBuf) -> Result<(), String> {
    match STORAGE_DIR.set(path.clone()) {
        Ok(()) => Ok(()),
        Err(_) if STORAGE_DIR.get() == Some(&path) => Ok(()),
        Err(_) => Err("语音服务配置目录已初始化为其他路径".into()),
    }
}

fn settings_path() -> Result<PathBuf, String> {
    Ok(STORAGE_DIR.get().ok_or("语音服务配置目录尚未初始化")?.join(FILE_NAME))
}

/// 本地服务只能在桌面应用自己的数据目录写模型、worker 与运行时；
/// 不使用 HOME 或工作区，避免影响用户其他 Python/模型安装。
pub fn storage_dir() -> Result<PathBuf, String> {
    STORAGE_DIR.get().cloned().ok_or("语音服务配置目录尚未初始化".into())
}

fn legacy_path() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(|home| Path::new(&home).join(".hachimo-local-asr.json"))
}

fn normalized_provider(value: &str, valid: &[&str], fallback: &str) -> String {
    let value = value.trim().to_ascii_lowercase();
    valid.contains(&value.as_str()).then_some(value).unwrap_or_else(|| fallback.into())
}

fn nonempty(value: Option<&str>, fallback: &str) -> String {
    value.map(str::trim).filter(|value| !value.is_empty()).unwrap_or(fallback).to_string()
}

fn normalize(mut preferences: VoiceServicePreferences) -> VoiceServicePreferences {
    preferences.version = 1;
    preferences.asr.provider = normalized_provider(&preferences.asr.provider, &["volcengine", "local"], "volcengine");
    preferences.asr.endpoint = nonempty(Some(&preferences.asr.endpoint), DEFAULT_ASR_ENDPOINT);
    preferences.asr.engine = nonempty(Some(&preferences.asr.engine), "sensevoice");
    preferences.tts.provider = normalized_provider(&preferences.tts.provider, &["volcengine", "macos", "kokoro"], "volcengine");
    preferences.tts.macos_voice = nonempty(Some(&preferences.tts.macos_voice), crate::local_tts::DEFAULT_VOICE);
    preferences.tts.kokoro_endpoint = nonempty(Some(&preferences.tts.kokoro_endpoint), DEFAULT_KOKORO_ENDPOINT);
    preferences.tts.kokoro_voice = nonempty(Some(&preferences.tts.kokoro_voice), "zf_001");
    /* Docker 版的占位音色名不属于原生 v1.1 的 103 个 speaker；升级时
     * 显式迁移，避免下拉框没有当前值或静默回退到未知音色。 */
    if preferences.tts.kokoro_voice == "zf_xiaobei" {
        preferences.tts.kokoro_voice = "zf_001".into();
    }
    preferences
}

fn migrate_legacy(path: &Path) -> VoiceServicePreferences {
    let Ok(raw) = std::fs::read_to_string(path) else { return VoiceServicePreferences::default() };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else { return VoiceServicePreferences::default() };
    let mut preferences = VoiceServicePreferences::default();
    preferences.asr.provider = value.get("provider").and_then(|v| v.as_str()).unwrap_or("volcengine").into();
    preferences.asr.endpoint = value.get("endpoint").and_then(|v| v.as_str()).unwrap_or(DEFAULT_ASR_ENDPOINT).into();
    preferences.asr.engine = value.get("engine").and_then(|v| v.as_str()).unwrap_or("sensevoice").into();
    preferences.tts.provider = match value.get("ttsProvider").and_then(|v| v.as_str()) {
        Some("local") => "macos".into(), Some("kokoro") => "kokoro".into(), _ => "volcengine".into(),
    };
    preferences.tts.macos_voice = value.get("localTtsVoice").and_then(|v| v.as_str())
        .unwrap_or(crate::local_tts::DEFAULT_VOICE).into();
    normalize(preferences)
}

fn save_at(dir: &Path, preferences: &VoiceServicePreferences) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|error| format!("创建语音服务配置目录失败：{error}"))?;
    let mut temp = tempfile::NamedTempFile::new_in(dir).map_err(|error| format!("创建语音服务配置临时文件失败：{error}"))?;
    let encoded = serde_json::to_vec_pretty(&normalize(preferences.clone())).map_err(|error| error.to_string())?;
    temp.write_all(&encoded).and_then(|_| temp.as_file().sync_all())
        .map_err(|error| format!("写入语音服务配置失败：{error}"))?;
    temp.persist(dir.join(FILE_NAME)).map_err(|error| format!("保存语音服务配置失败：{}", error.error))?;
    Ok(())
}

fn load_at(dir: &Path, legacy: &Path) -> Result<VoiceServicePreferences, String> {
    let path = dir.join(FILE_NAME);
    if path.exists() {
        let raw = std::fs::read(&path).map_err(|error| format!("读取语音服务配置失败：{error}"))?;
        return serde_json::from_slice::<VoiceServicePreferences>(&raw).map(normalize)
            .map_err(|_| "语音服务配置格式损坏；请保留原文件".into());
    }
    let migrated = migrate_legacy(legacy);
    save_at(dir, &migrated)?;
    Ok(migrated)
}

pub fn load() -> Result<VoiceServicePreferences, String> {
    let Some(path) = STORAGE_DIR.get().map(|dir| dir.join(FILE_NAME)) else {
        // Rust unit tests and early startup may run before Tauri exposes app-data.
        // Read-only legacy fallback keeps existing call sites deterministic; only
        // configured production storage is allowed to persist a migration.
        return Ok(legacy_path().as_deref().map(migrate_legacy).unwrap_or_default());
    };
    let legacy = legacy_path().unwrap_or_else(|| path.with_file_name("no-legacy-settings.json"));
    load_at(path.parent().ok_or("语音服务配置目录无效")?, &legacy)
}

pub fn save(preferences: VoiceServicePreferences) -> Result<VoiceServicePreferences, String> {
    let path = settings_path()?;
    let preferences = normalize(preferences);
    save_at(path.parent().ok_or("语音服务配置目录无效")?, &preferences)?;
    Ok(preferences)
}

pub fn update(input: VoiceServicePreferencesInput) -> Result<VoiceServicePreferences, String> {
    let mut preferences = load()?;
    if let Some(provider) = input.asr_provider { preferences.asr.provider = provider; }
    if let Some(endpoint) = input.local_asr_endpoint { preferences.asr.endpoint = endpoint; }
    if let Some(engine) = input.local_asr_engine { preferences.asr.engine = engine; }
    if let Some(provider) = input.tts_provider { preferences.tts.provider = provider; }
    if let Some(voice) = input.macos_tts_voice { preferences.tts.macos_voice = voice; }
    if let Some(endpoint) = input.kokoro_endpoint { preferences.tts.kokoro_endpoint = endpoint; }
    if let Some(voice) = input.kokoro_voice { preferences.tts.kokoro_voice = voice; }
    save(preferences)
}

pub fn local_asr() -> Option<(String, String)> {
    let preferences = load().ok()?;
    (preferences.asr.provider == "local").then_some((preferences.asr.endpoint, preferences.asr.engine))
}

pub fn tts_provider() -> String { load().map(|preferences| preferences.tts.provider).unwrap_or_else(|_| "volcengine".into()) }
pub fn macos_voice() -> String { load().map(|preferences| preferences.tts.macos_voice).unwrap_or_else(|_| crate::local_tts::DEFAULT_VOICE.into()) }

#[tauri::command]
pub fn voice_service_preferences() -> Result<VoiceServicePreferences, String> { load() }

#[tauri::command]
pub fn save_voice_service_preferences(input: VoiceServicePreferencesInput) -> Result<VoiceServicePreferences, String> { update(input) }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_legacy_local_asr_and_macos_tts_preferences() {
        let temp = tempfile::tempdir().unwrap();
        let legacy = temp.path().join("legacy.json");
        std::fs::write(&legacy, r#"{"provider":"local","endpoint":"http://127.0.0.1:8900","engine":"qwen3","ttsProvider":"local","localTtsVoice":"Tingting"}"#).unwrap();
        let preferences = load_at(temp.path(), &legacy).unwrap();
        assert_eq!(preferences.asr.provider, "local");
        assert_eq!(preferences.asr.engine, "qwen3");
        assert_eq!(preferences.tts.provider, "macos");
        assert_eq!(preferences.tts.macos_voice, "Tingting");
    }

    #[test]
    fn saving_cloud_asr_selection_replaces_migrated_local_selection() {
        let temp = tempfile::tempdir().unwrap();
        let legacy = temp.path().join("legacy.json");
        std::fs::write(&legacy, r#"{"provider":"local"}"#).unwrap();
        let mut preferences = load_at(temp.path(), &legacy).unwrap();
        preferences.asr.provider = "volcengine".into();
        save_at(temp.path(), &preferences).unwrap();
        assert_eq!(load_at(temp.path(), &legacy).unwrap().asr.provider, "volcengine");
    }

    #[test]
    fn migrates_the_retired_kokoro_voice_name_to_the_native_model_voice() {
        let mut preferences = VoiceServicePreferences::default();
        preferences.tts.provider = "kokoro".into();
        preferences.tts.kokoro_voice = "zf_xiaobei".into();
        assert_eq!(normalize(preferences).tts.kokoro_voice, "zf_001");
    }
}
