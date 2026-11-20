use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const MODEL_NAME: &str = "kokoro-multi-lang-v1_1";
pub const MODEL_ARCHIVE_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/kokoro-multi-lang-v1_1.tar.bz2";
const PORT: u16 = 8891;
const DEFAULT_ENDPOINT: &str = "http://127.0.0.1:8891";
/// 模型下载、Python venv 建立及首次内存映射均可能较慢。
const COLD_START_TIMEOUT: Duration = Duration::from_secs(300);
/// 本地模型第一次推理允许较长时间，不能当作云服务超时处理。
const SYNTHESIS_TIMEOUT: Duration = Duration::from_secs(180);
const WORKER_SCRIPT: &str = include_str!("../../resources/kokoro_server.py");

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KokoroStatus {
    pub ready: bool,
    pub installed: bool,
    pub message: String,
    pub endpoint: String,
}

fn endpoint() -> String {
    crate::voice_service_settings::load()
        .map(|prefs| prefs.tts.kokoro_endpoint)
        .unwrap_or_else(|_| DEFAULT_ENDPOINT.into())
        .trim_end_matches('/')
        .to_string()
}

fn root_dir() -> Result<PathBuf, String> {
    crate::voice_service_settings::storage_dir().map(|dir| dir.join("kokoro"))
}

fn model_root() -> Result<PathBuf, String> { Ok(root_dir()?.join("models")) }
fn model_dir() -> Result<PathBuf, String> { Ok(model_root()?.join(MODEL_NAME)) }
fn worker_script() -> Result<PathBuf, String> { Ok(root_dir()?.join("kokoro_server.py")) }
fn runtime_dir() -> Result<PathBuf, String> { Ok(root_dir()?.join("runtime")) }

fn expected_model_layout(path: &Path) -> bool {
    ["model.onnx", "voices.bin", "tokens.txt", "lexicon-us-en.txt", "lexicon-zh.txt"]
        .iter()
        .all(|name| path.join(name).is_file())
        && path.join("espeak-ng-data").is_dir()
}

fn validate_wav(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() <= 44 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("Kokoro 返回的不是有效 WAV 音频".into());
    }
    let channels = u16::from_le_bytes([bytes[22], bytes[23]]);
    let sample_rate = u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]);
    if channels != 1 || sample_rate != 16_000 {
        return Err("Kokoro 返回的音频不是设备要求的 16kHz 单声道 PCM".into());
    }
    Ok(())
}

fn health(endpoint: &str) -> bool {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(900))
        .build()
        .ok()
        .and_then(|client| client.get(format!("{endpoint}/health")).send().ok())
        .is_some_and(|response| response.status().is_success())
}

pub fn is_official_model_archive(url: &str) -> bool {
    url == MODEL_ARCHIVE_URL
        && url.starts_with("https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/")
        && url.ends_with(".tar.bz2")
}

pub fn native_worker_uses_docker() -> bool { false }

fn python_candidates() -> [&'static str; 3] {
    ["/opt/homebrew/opt/python@3.14/bin/python3.14", "/usr/bin/python3", "python3"]
}

fn system_python() -> Result<String, String> {
    for python in python_candidates() {
        if Command::new(python).args(["-c", "import sys; assert sys.version_info >= (3, 9)"]).status().is_ok_and(|status| status.success()) {
            return Ok(python.into());
        }
    }
    Err("未找到 Python 3.9 或更新版本；本地 Kokoro 需要 Python 运行时".into())
}

fn supports_sherpa(python: &Path) -> bool {
    Command::new(python)
        .args(["-c", "import sherpa_onnx; assert hasattr(sherpa_onnx, 'OfflineTts')"])
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(target_os = "windows")]
fn runtime_python_path(runtime: &Path) -> PathBuf { runtime.join("Scripts").join("python.exe") }
#[cfg(not(target_os = "windows"))]
fn runtime_python_path(runtime: &Path) -> PathBuf { runtime.join("bin").join("python") }

fn run_checked(mut command: Command, purpose: &str) -> Result<(), String> {
    let output = command.output().map_err(|error| format!("{purpose}无法启动：{error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(if stderr.is_empty() { format!("{purpose}失败") } else { format!("{purpose}失败：{stderr}") })
}

fn ensure_runtime() -> Result<PathBuf, String> {
    let runtime = runtime_dir()?;
    let python = runtime_python_path(&runtime);
    if python.is_file() && supports_sherpa(&python) {
        return Ok(python);
    }
    /* 优先复用本地 ASR 已安装的原生推理运行时，避免同一台电脑
     * 为 ASR/TTS 各放一份 onnxruntime；本机没有时才创建私有 venv。 */
    for candidate in python_candidates() {
        let candidate = PathBuf::from(candidate);
        if supports_sherpa(&candidate) {
            return Ok(candidate);
        }
    }
    if !python.is_file() {
        let host_python = system_python()?;
        let mut create = Command::new(host_python);
        create.args(["-m", "venv"]).arg(&runtime);
        run_checked(create, "创建本地 Kokoro 运行环境")?;
    }
    let mut install = Command::new(&python);
    install.args(["-m", "pip", "install", "--disable-pip-version-check", "--no-input", "sherpa-onnx==2.0.1"]);
    run_checked(install, "安装本地 Kokoro 运行时")?;
    let mut verify = Command::new(&python);
    verify.args(["-c", "import sherpa_onnx; assert hasattr(sherpa_onnx, 'OfflineTts')"]);
    run_checked(verify, "验证本地 Kokoro 运行时")?;
    Ok(python)
}

fn write_worker_script() -> Result<PathBuf, String> {
    let path = worker_script()?;
    let parent = path.parent().ok_or("Kokoro 工作目录无效")?;
    std::fs::create_dir_all(parent).map_err(|error| format!("创建 Kokoro 工作目录失败：{error}"))?;
    if std::fs::read_to_string(&path).ok().as_deref() != Some(WORKER_SCRIPT) {
        let temp = tempfile::NamedTempFile::new_in(parent).map_err(|error| format!("创建 Kokoro worker 临时文件失败：{error}"))?;
        std::fs::write(temp.path(), WORKER_SCRIPT).map_err(|error| format!("写入 Kokoro worker 失败：{error}"))?;
        temp.persist(&path).map_err(|error| format!("保存 Kokoro worker 失败：{}", error.error))?;
    }
    Ok(path)
}

fn status_with(endpoint: String, installed: bool) -> KokoroStatus {
    let ready = health(&endpoint);
    let message = match (installed, ready) {
        (_, true) => "Kokoro 本地模型已就绪".into(),
        (false, false) => "Kokoro 模型未安装；请点击“下载并安装”".into(),
        (true, false) => "Kokoro 模型已安装但未启动".into(),
    };
    KokoroStatus { ready, installed, message, endpoint }
}

pub fn status() -> KokoroStatus {
    let endpoint = endpoint();
    let installed = model_dir().is_ok_and(|dir| expected_model_layout(&dir));
    status_with(endpoint, installed)
}

fn should_auto_start(provider: &str, installed: bool) -> bool {
    provider == "kokoro" && installed
}

/// 客户端重启后恢复已选中的本地 TTS。未安装时不做任何下载或等待，
/// 以免首次配置影响桌面端的正常启动。
pub fn auto_start_if_configured() -> Result<bool, String> {
    let current = status();
    if !should_auto_start(&crate::voice_service_settings::tts_provider(), current.installed) {
        return Ok(false);
    }
    kokoro_tts_start()?;
    Ok(true)
}

#[tauri::command]
pub fn kokoro_tts_status() -> KokoroStatus { status() }

#[tauri::command]
pub fn kokoro_tts_install() -> Result<KokoroStatus, String> {
    if !is_official_model_archive(MODEL_ARCHIVE_URL) || native_worker_uses_docker() {
        return Err("Kokoro 安装源配置无效".into());
    }
    let python = ensure_runtime()?;
    let script = write_worker_script()?;
    let root = model_root()?;
    let mut install = Command::new(python);
    install.arg(&script).arg("--install").arg("--model-root").arg(root);
    run_checked(install, "下载并安装 Kokoro 模型")?;
    if !model_dir().is_ok_and(|dir| expected_model_layout(&dir)) {
        return Err("Kokoro 模型安装后未通过完整性检查".into());
    }
    Ok(status())
}

#[tauri::command]
pub fn kokoro_tts_start() -> Result<KokoroStatus, String> {
    let endpoint = endpoint();
    if health(&endpoint) { return Ok(status()); }
    let model = model_dir()?;
    if !expected_model_layout(&model) {
        return Err("Kokoro 模型未安装；请先点击“下载并安装”".into());
    }
    let python = ensure_runtime()?;
    let script = write_worker_script()?;
    let root = model_root()?;
    let log = std::fs::File::create(root_dir()?.join("kokoro.log"))
        .map_err(|error| format!("创建 Kokoro 日志失败：{error}"))?;
    Command::new(python)
        .arg(script)
        .arg("--serve")
        .arg("--model-root")
        .arg(root)
        .arg("--port")
        .arg(PORT.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone().map_err(|error| error.to_string())?))
        .stderr(Stdio::from(log))
        .spawn()
        .map_err(|error| format!("启动 Kokoro 原生 worker 失败：{error}"))?;
    let deadline = Instant::now() + COLD_START_TIMEOUT;
    while Instant::now() < deadline {
        if health(&endpoint) { return Ok(status()); }
        std::thread::sleep(Duration::from_millis(400));
    }
    Err("Kokoro 启动超时；请查看设置页提示或稍后重试".into())
}

#[tauri::command]
pub fn kokoro_tts_stop() -> Result<KokoroStatus, String> {
    let endpoint = endpoint();
    if !health(&endpoint) { return Ok(status()); }
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .map_err(|error| error.to_string())?
        .post(format!("{endpoint}/shutdown"))
        .send()
        .map_err(|error| format!("停止 Kokoro 失败：{error}"))?;
    let deadline = Instant::now() + Duration::from_secs(4);
    while Instant::now() < deadline {
        if !health(&endpoint) { return Ok(status()); }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err("Kokoro 停止超时".into())
}

fn voice_to_sid(voice: &str) -> i32 {
    match voice.trim() {
        "zf_001" | "zf_xiaobei" => 3,
        "zf_002" => 4,
        "zf_003" => 5,
        "zf_004" => 6,
        "zf_005" => 7,
        "zm_009" => 58,
        "zm_010" => 59,
        raw => raw.parse::<i32>().ok().filter(|sid| (0..=102).contains(sid)).unwrap_or(3),
    }
}

pub fn synthesize(text: &str, voice: &str, speed: f32) -> Result<crate::local_tts::TtsResult, String> {
    let endpoint = endpoint();
    if !health(&endpoint) {
        return Err("Kokoro 未启动；请在 API 设置中下载模型并启动本地轻量 TTS".into());
    }
    let started = Instant::now();
    let response = reqwest::blocking::Client::builder()
        .timeout(SYNTHESIS_TIMEOUT)
        .build()
        .map_err(|error| error.to_string())?
        .post(format!("{endpoint}/tts"))
        .json(&serde_json::json!({"text": text.trim(), "sid": voice_to_sid(voice), "speed": speed.clamp(0.25, 4.0)}))
        .send()
        .map_err(|error| format!("Kokoro 请求失败：{error}"))?;
    if !response.status().is_success() {
        return Err(format!("Kokoro 合成失败：HTTP {}", response.status()));
    }
    let bytes = response.bytes().map_err(|error| format!("读取 Kokoro 音频失败：{error}"))?.to_vec();
    validate_wav(&bytes)?;
    Ok(crate::local_tts::TtsResult { wav: bytes, voice: voice.into(), ms: started.elapsed().as_millis() as u64 })
}

/// 设置页试听：与设备实际发送完全共用 synthesize，避免“试听成功但设备没有声音”。
#[tauri::command]
pub async fn kokoro_tts_preview(
    text: String,
    voice: Option<String>,
) -> Result<crate::local_tts::LocalTtsPreview, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let voice = voice.unwrap_or_else(|| "zf_001".into());
        let result = synthesize(&text, &voice, 1.0)?;
        use base64::Engine as _;
        Ok(crate::local_tts::LocalTtsPreview {
            bytes: result.wav.len(),
            voice: result.voice,
            ms: result.ms,
            wav_base64: base64::engine::general_purpose::STANDARD.encode(result.wav),
        })
    })
    .await
    .map_err(|error| format!("Kokoro 试听任务失败：{error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_nonempty_riff_wav_responses() {
        assert!(validate_wav(b"RIFFxxxxWAVE").is_err());
        let mut wav = vec![0; 48];
        wav[..4].copy_from_slice(b"RIFF");
        wav[8..12].copy_from_slice(b"WAVE");
        wav[22..24].copy_from_slice(&1u16.to_le_bytes());
        wav[24..28].copy_from_slice(&16_000u32.to_le_bytes());
        assert!(validate_wav(&wav).is_ok());
        wav[24..28].copy_from_slice(&24_000u32.to_le_bytes());
        assert!(validate_wav(&wav).is_err());
    }

    #[test]
    fn native_installer_uses_the_official_multilingual_model_without_docker() {
        assert!(is_official_model_archive(MODEL_ARCHIVE_URL));
        assert!(!native_worker_uses_docker());
    }

    #[test]
    fn local_model_timeouts_allow_cold_start() {
        assert!(COLD_START_TIMEOUT >= Duration::from_secs(180));
        assert!(SYNTHESIS_TIMEOUT >= Duration::from_secs(120));
    }

    #[test]
    fn selected_chinese_voice_names_resolve_to_official_speaker_ids() {
        assert_eq!(voice_to_sid("zf_001"), 3);
        assert_eq!(voice_to_sid("zm_009"), 58);
        assert_eq!(voice_to_sid("zm_010"), 59);
    }

    #[test]
    fn auto_start_only_applies_to_an_installed_selected_kokoro_service() {
        assert!(should_auto_start("kokoro", true));
        assert!(!should_auto_start("kokoro", false));
        assert!(!should_auto_start("macos", true));
        assert!(!should_auto_start("volcengine", true));
    }
}
