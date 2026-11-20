/*
 * [Input] 设备端 PTT（pet_p4_voice_ptt.c）录完一段话后把整段 WAV
 *         HTTP POST 到 PC 的 http://<PC-IP>:8901/voice（固件默认端点，
 *         NVS 可覆盖）。body 是裸 WAV 字节，Content-Type: audio/wav。
 * [Output] {"ok":true,"text":"…","engine":"…","ms":…}；失败
 *         {"ok":false,"error":"…"}。与本地 ASR 服务（:8900/stt）同款契约。
 * [Pos] PC 端常驻桥接：App 启动即自动监听 0.0.0.0:8901，把设备 PTT 的
 *       WAV 转给当前选择的识别来源（本地 ASR）。这是移植时缺失的一环——
 *       原项目由 tools/voice/bridge.py 承担，本仓库没有这个脚本。
 * [Sync] 固件侧契约见 firmware/main/pet_p4_voice_ptt.c（PTT_URL_DEFAULT、
 *        ptt_worker_task 的响应解析）；改响应字段要同步固件。
 *
 * 为什么不做成 Tauri command：设备是独立 HTTP 客户端，不走 WebView，
 * 必须由 Rust 侧自己开端口。用 std::net 手写最小 HTTP/1.1（只收
 * POST /voice），不引入服务框架依赖。
 */

use std::io::{Read, Write};
use std::collections::HashMap;
use std::net::{IpAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex, OnceLock};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tauri::Emitter;

const BIND_ADDR: &str = "0.0.0.0:8901";
/// 固件 PTT PCM 缓冲上限 512 KiB，WAV 头固定 44 字节。
const MAX_BODY: usize = 512 * 1024 + 44;
const VOICE_MAX_CONNECTIONS: usize = 4;
const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// 本地 ASR 冷加载 + 识别的最坏情况（Qwen3 冷启动约 19s）。
const ASR_TIMEOUT: Duration = Duration::from_secs(45);

/// WiFi PTT 目标冻结（round-5，对齐 USB 路径 DeviceVoiceContext 语义）：
/// 固件在按下语音键瞬间 POST /voice/begin，这里快照「当时跟随的 Agent」；
/// 录完投递前若跟随目标已变化，则丢弃本次识别文本，宁可丢字也不写错地方。
#[derive(Clone, Debug)]
struct VoiceBeginSnapshot {
    agent_id: String,
    at: Instant,
    target: Option<WifiVoiceTarget>,
}

#[derive(Clone, Debug)]
struct WifiVoiceTarget {
    board: String,
    agent: String,
    session: String,
    generation: u64,
}

impl WifiVoiceTarget {
    fn from_binding(agent: &str, binding: &crate::P4SessionBinding) -> Option<Self> {
        // Visible-app delivery currently resolves the foreground window, not
        // a session ID; it must not pretend to pin a specific conversation.
        if matches!(agent, "workbuddy" | "deepseek-harness") || agent.is_empty()
            || binding.agent_id != agent || binding.board_device_id.is_empty()
            || binding.session_id.trim().is_empty()
            || matches!(binding.session_id.trim().to_ascii_lowercase().as_str(), "auto" | "new")
            || binding.auto_follow || binding.generation == 0 {
            return None;
        }
        Some(Self { board: binding.board_device_id.clone(), agent: agent.into(),
            session: binding.session_id.clone(), generation: binding.generation })
    }
    fn matches(&self, binding: &crate::P4SessionBinding) -> bool {
        !binding.auto_follow && binding.board_device_id == self.board && binding.agent_id == self.agent
            && binding.session_id == self.session && binding.generation == self.generation
    }
}

const VOICE_BEGIN_SNAPSHOT_TTL: Duration = Duration::from_secs(120);
const VOICE_BEGIN_MAX_PENDING: usize = 16;
const VOICE_BEGIN_WAIT: Duration = Duration::from_secs(3);

#[derive(Default)]
struct VoiceSnapshotStore {
    pending: HashMap<(IpAddr, String), VoiceBeginSnapshot>,
}

fn valid_voice_id(id: &str) -> bool {
    id.len() == 16 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl VoiceSnapshotStore {
    fn insert(&mut self, peer: IpAddr, id: &str, agent: &str, now: Instant) -> bool {
        if !valid_voice_id(id) || agent.is_empty() { return false; }
        self.pending.retain(|_, entry| now.saturating_duration_since(entry.at) <= VOICE_BEGIN_SNAPSHOT_TTL);
        let key = (peer, id.to_owned());
        if self.pending.contains_key(&key) { return false; }
        // Short/cancelled recordings never upload, so their snapshots may fill
        // the bounded store. Evict the oldest instead of blocking all new PTT
        // attempts for two minutes; an evicted recording fails closed at take().
        if self.pending.len() >= VOICE_BEGIN_MAX_PENDING {
            if let Some(oldest) = self.pending.iter().min_by_key(|(_, entry)| entry.at).map(|(key, _)| key.clone()) {
                self.pending.remove(&oldest);
            }
        }
        self.pending.insert(key, VoiceBeginSnapshot { agent_id: agent.to_owned(), at: now, target: None });
        true
    }

    fn take_scoped(&mut self, peer: IpAddr, id: &str, now: Instant) -> Option<VoiceBeginSnapshot> {
        let snapshot = self.pending.remove(&(peer, id.to_owned()))?;
        (now.saturating_duration_since(snapshot.at) <= VOICE_BEGIN_SNAPSHOT_TTL)
            .then_some(snapshot)
    }
    #[cfg(test)]
    fn take(&mut self, peer: IpAddr, id: &str, now: Instant) -> Option<String> {
        self.take_scoped(peer, id, now).map(|snapshot| snapshot.agent_id)
    }
}

static VOICE_BEGIN_SNAPSHOTS: OnceLock<Mutex<VoiceSnapshotStore>> = OnceLock::new();

fn voice_snapshots() -> &'static Mutex<VoiceSnapshotStore> {
    VOICE_BEGIN_SNAPSHOTS.get_or_init(|| Mutex::new(VoiceSnapshotStore::default()))
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default()
}

/// 当前跟随的 Agent（与 PC 前端「跟随」同一数据源：bridge profile）。
fn followed_agent_id() -> String {
    let profile = crate::get_bridge_profile_path()
        .ok()
        .and_then(|path| crate::read_bridge_profile(&path).ok())
        .flatten();
    match profile {
        Some(raw) => {
            let profile = crate::normalize_bridge_profile(raw);
            if !profile.selected_agent_id.is_empty() {
                profile.selected_agent_id
            } else {
                profile
                    .enabled_agents
                    .first()
                    .cloned()
                    .unwrap_or_default()
            }
        }
        None => String::new(),
    }
}

fn store_voice_begin_snapshot(peer: IpAddr, id: &str) -> bool {
    let agent = followed_agent_id();
    let target = crate::p4_session_binding().lock().ok()
        .and_then(|binding| WifiVoiceTarget::from_binding(&agent, &binding));
    voice_snapshots().lock().map(|mut store| {
        if !store.insert(peer, id, &agent, Instant::now()) { return false; }
        if let Some(snapshot) = store.pending.get_mut(&(peer, id.to_owned())) { snapshot.target = target; }
        true
    }).unwrap_or(false)
}

fn take_voice_begin_info(peer: IpAddr, id: &str) -> Option<VoiceBeginSnapshot> {
    let deadline = Instant::now() + VOICE_BEGIN_WAIT;
    loop {
        let snapshot = voice_snapshots().lock().ok()?.take_scoped(peer, id, Instant::now());
        if snapshot.is_some() || Instant::now() >= deadline { return snapshot; }
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(test)]
fn take_voice_begin_snapshot(peer: IpAddr, id: &str) -> Option<String> {
    take_voice_begin_info(peer, id).map(|snapshot| snapshot.agent_id)
}

fn voice_request_id(query: &str) -> Option<&str> {
    let mut ids = query.split('&').filter_map(|part| part.strip_prefix("sid="));
    let id = ids.next()?;
    if ids.next().is_some() || !valid_voice_id(id) { return None; }
    Some(id)
}

/// ASR 识别完成后先生成待确认草稿，设备摇杆向上发送，向下取消。
#[derive(Clone, Debug, serde::Serialize)]
pub struct VoiceDraft {
    pub text: String,
    pub agent_id: String,
    pub utterance_id: String,
    pub at_ms: u128,
    #[serde(skip)]
    target: Option<WifiVoiceTarget>,
}

/// 草稿有效窗口（录音上限 + 确认操作时间）。
pub const VOICE_DRAFT_TTL_MS: u128 = 180_000;

static VOICE_DRAFT: Mutex<Option<VoiceDraft>> = Mutex::new(None);

pub fn current_draft() -> Option<VoiceDraft> {
    let mut guard = VOICE_DRAFT.lock().ok()?;
    let draft = guard.clone()?;
    if now_ms().saturating_sub(draft.at_ms) > VOICE_DRAFT_TTL_MS {
        *guard = None;
        return None;
    }
    Some(draft)
}

pub fn draft_status() -> serde_json::Value {
    match current_draft() {
        Some(draft) => serde_json::json!({
            "ok": true,
            "pending": true,
            "text": draft.text,
            "agentId": draft.agent_id,
            "utteranceId": draft.utterance_id,
            "tsMs": draft.at_ms as u64,
        }),
        None => serde_json::json!({"ok": true, "pending": false}),
    }
}

fn emit_voice_event(app: &tauri::AppHandle, event: &str, payload: serde_json::Value) {
    let _ = app.emit(event, payload.clone());
    bridge_log(&format!("emit {event}"));
}

/// 通知设备进入/退出「语音草稿确认模式」：期间设备本地屏蔽左右摇杆等
/// 原有设定，并在屏幕上显示发送/取消提示。走原生 USB/TCP 协议行，
/// 设备不在线时只记日志，不影响 PC 端草稿。
fn notify_device_draft(app: &tauri::AppHandle, active: bool, text: &str) {
    use tauri::Manager;
    let Some(manager) = app.try_state::<crate::usb_serial::UsbSerialManager>() else {
        return;
    };
    let payload = serde_json::json!({
        "active": active,
        "text": text,
        "hint": "语音待确认：摇杆向上发送 · 向下取消",
    });
    match manager.send("voice/draft", &payload) {
        Ok(()) => bridge_log(&format!("voice/draft notify active={active} sent")),
        Err(error) => bridge_log(&format!("voice/draft notify active={active} failed: {error}")),
    }
}

fn notify_unbound_voice_target(app: &tauri::AppHandle, agent: &str) {
    use tauri::Manager;
    if let Some(manager) = app.try_state::<crate::usb_serial::UsbSerialManager>() {
        let status = manager.status();
        let session = crate::p4_session_binding().lock().ok().and_then(|binding|
            (binding.board_device_id == status.board_device_id && binding.agent_id == agent)
                .then(|| binding.session_id.clone()));
        if let (Ok(generation), Some(session)) = (status.connection_id.parse::<u64>(), session) {
            let notice = crate::display_only_voice_notice("会话未固定或已变化，未发送", agent, &session);
            let _ = manager.send_voice_notice_for_connection(&status.board_device_id, generation, &status.boot_id, &notice);
        }
    }
}

fn create_draft(app: &tauri::AppHandle, text: &str, agent_id: &str, target: Option<WifiVoiceTarget>) -> Result<VoiceDraft, String> {
    if !crate::workbuddy_device_draft_fits(text.trim()) || agent_id.is_empty() {
        return Err("文字过长、含换行或目标不可用，请缩短后重新录音".into());
    }
    let stamp = now_ms();
    let draft = VoiceDraft {
        text: text.trim().to_string(),
        agent_id: agent_id.to_string(),
        utterance_id: format!("wifi-draft-{stamp}"),
        at_ms: stamp,
        target,
    };
    let mut guard = VOICE_DRAFT.lock().map_err(|_| "语音草稿状态不可用".to_string())?;
    *guard = Some(draft.clone());
    emit_voice_event(
        app,
        "voice-draft-ready",
        serde_json::json!({
            "text": draft.text,
            "agentId": draft.agent_id,
            "utteranceId": draft.utterance_id,
            "tsMs": stamp as u64,
        }),
    );
    notify_device_draft(app, true, &draft.text);
    bridge_log(&format!(
        "voice draft ready (agent=\"{}\", {} chars) — awaiting send/cancel",
        draft.agent_id,
        draft.text.chars().count()
    ));
    drop(guard);
    Ok(draft)
}

pub fn clear_draft(app: &tauri::AppHandle, reason: &str) {
    if let Ok(mut guard) = VOICE_DRAFT.lock() {
      if guard.take().is_some() {
        notify_device_draft(app, false, reason);
        emit_voice_event(app, "voice-draft-cleared", serde_json::json!({"reason": reason}));
        bridge_log(&format!("voice draft cleared: {reason}"));
      }
    }
}

fn validate_draft_submission(draft: &VoiceDraft, current: &str, now: u128) -> Result<(), (&'static str, String)> {
    if now < draft.at_ms || now - draft.at_ms > VOICE_DRAFT_TTL_MS {
        return Err(("草稿已超时", "语音草稿已超时，请重新说一次".into()));
    }
    if current.is_empty() || current != draft.agent_id {
        return Err(("跟随的 Agent 不可用或已切换", "跟随的 Agent 不可用或已切换，草稿已取消，请重新录音".into()));
    }
    if draft.text.trim().is_empty() {
        return Err(("草稿为空", "语音草稿为空".into()));
    }
    Ok(())
}

pub fn submit_draft(app: &tauri::AppHandle) -> Result<serde_json::Value, String> {
    let mut guard = VOICE_DRAFT.lock().map_err(|_| "语音草稿状态不可用".to_string())?;
    let draft = guard.take().ok_or_else(|| "没有待发送的语音草稿".to_string())?;
    let current = followed_agent_id();
    if let Err((reason, error)) = validate_draft_submission(&draft, &current, now_ms()) {
        notify_device_draft(app, false, reason);
        emit_voice_event(app, "voice-draft-cleared", serde_json::json!({"reason": reason}));
        return Err(error);
    }
    let Some(target) = draft.target.clone().filter(|target|
        crate::p4_session_binding().lock().is_ok_and(|binding| target.matches(&binding))) else {
        notify_device_draft(app, false, "会话未固定或已变化");
        notify_unbound_voice_target(app, &draft.agent_id);
        emit_voice_event(app, "voice-draft-cleared", serde_json::json!({"reason": "会话未固定或已变化"}));
        return Err("无线语音无法确认原会话，本次未发送；请选择明确会话或使用 USB 语音".into());
    };
    bridge_log(&format!(
        "voice draft submitting -> agent=\"{}\" chars={}",
        draft.agent_id,
        draft.text.chars().count()
    ));
    notify_device_draft(app, false, "submitting");
    emit_voice_event(app, "voice-draft-cleared", serde_json::json!({"reason": "submitting"}));
    drop(guard);
    // Agent 注入可能耗时数分钟（CLI 冷启动 + 整轮回复）。同步等待会让
    // 「发送」按钮和摇杆手势卡死，因此后台执行，结果经事件回报前端。
    let app = app.clone();
    let text = draft.text.clone();
    let agent_id = draft.agent_id.clone();
    std::thread::spawn(move || {
        if !crate::p4_session_binding().lock().is_ok_and(|binding| target.matches(&binding)) {
            emit_voice_event(&app, "voice-draft-result", serde_json::json!({
                "ok": false, "message": "会话已变化，本次未发送", "agentId": agent_id,
            }));
            return;
        }
        match crate::direct_inject_agent_input_to_session(&text, &agent_id, &target.session) {
            Ok(response) => {
                let queued = response.get("queued").and_then(|v| v.as_bool()).unwrap_or(false);
                let message = if queued {
                    format!(
                        "已提交，但 {} 正忙：桥最多重试 3 次（约 10 秒）。若未送达请切换 Agent 或查看 ~/.claw-pet/logs/status-bridge.log",
                        agent_id
                    )
                } else {
                    format!("已向 {} 请求提交，未取得送达回执", agent_id)
                };
                emit_voice_event(&app, "voice-draft-result", serde_json::json!({
                    "ok": true, "queued": queued, "message": message,
                    "agentId": agent_id, "text": text,
                }));
                bridge_log(&format!("voice draft injected ok: {message}"));
            }
            Err(error) => {
                emit_voice_event(&app, "voice-draft-result", serde_json::json!({
                    "ok": false, "message": error, "agentId": agent_id, "text": text,
                }));
                bridge_log(&format!("voice draft inject FAILED: {error}"));
            }
        }
    });
    Ok(serde_json::json!({
        "ok": true,
        "submitted": false,
        "pending": true,
        "agentId": draft.agent_id,
        "text": draft.text,
    }))
}

pub fn cancel_draft(app: &tauri::AppHandle, reason: &str) -> bool {
    let existed = current_draft().is_some();
    clear_draft(app, reason);
    existed
}

fn wifi_draft_decision(topic: &str, payload: &serde_json::Value, board: &str) -> Option<bool> {
    if topic != "input/event" || board.is_empty()
        || payload["boardDeviceId"].as_str() != Some(board)
        || payload["handledLocally"].as_bool() != Some(false) { return None; }
    match (payload["event"].as_str(), payload["action"].as_str()) {
        (Some("joystick.up"), Some("voice_draft_send")) => Some(true),
        (Some("joystick.down"), Some("voice_draft_cancel")) => Some(false),
        _ => None,
    }
}

/// 草稿期间只由本设备摇杆上下决定，其他按键不再触发原有功能。
pub fn try_draft_gesture(app: &tauri::AppHandle, topic: &str, payload: &serde_json::Value) -> bool {
    if topic != "input/event" && topic != "input/action" {
        return false;
    }
    if current_draft().is_none() {
        return false;
    }
    use tauri::Manager;
    let board = app.try_state::<crate::usb_serial::UsbSerialManager>()
        .map(|manager| manager.status().board_device_id).unwrap_or_default();
    let action = payload.get("action").and_then(|v| v.as_str()).unwrap_or_default();
    if wifi_draft_decision(topic, payload, &board) == Some(true) {
        if let Err(error) = submit_draft(app) {
            emit_voice_event(app, "voice-draft-result", serde_json::json!({
                "ok": false, "message": error,
            }));
        }
    } else if wifi_draft_decision(topic, payload, &board) == Some(false) {
        clear_draft(app, "设备取消");
        emit_voice_event(app, "voice-draft-result", serde_json::json!({
            "ok": true, "cancelled": true, "message": "已取消语音草稿",
        }));
    } else if topic == "input/event" && action != "voice_ptt" {
        // 草稿确认窗口内暂时屏蔽其余所有按键的原有设定（PC 侧），
        // 设备侧 apply_local_action 也已同步屏蔽；两者都不再广播事件。
        bridge_log(&format!("voice draft: swallowed device action \"{action}\""));
        return true;
    } else {
        return false;
    }
    true
}

/// 每次设备 PTT 请求都落一行日志。eprintln 在 GUI 启动下看不到，
/// 排查「按了语音键没反应」必须有一份能读的记录：
/// 设备有没有连上来、发了多大、识别结果/错误是什么。
fn bridge_log(message: &str) {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let line = format!("{stamp} {message}\n");
    if let Ok(home) = std::env::var("HOME") {
        let path = std::path::Path::new(&home).join(".hachimo").join("voice-bridge.log");
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = file.write_all(line.as_bytes());
        }
    }
    eprintln!("[device-voice-bridge] {message}");
}

/// App 启动时调用一次；失败只打日志，不影响主程序。
pub fn start(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        if let Err(error) = serve(app) {
            eprintln!("[device-voice-bridge] {error}");
        }
    });
}

struct VoiceConnectionPermit(Arc<AtomicUsize>);

impl VoiceConnectionPermit {
    fn acquire(active: &Arc<AtomicUsize>) -> Option<Self> {
        active.fetch_update(Ordering::SeqCst, Ordering::SeqCst,
            |count| (count < VOICE_MAX_CONNECTIONS).then_some(count + 1)).ok()?;
        Some(Self(Arc::clone(active)))
    }
}

impl Drop for VoiceConnectionPermit {
    fn drop(&mut self) { self.0.fetch_sub(1, Ordering::SeqCst); }
}

fn serve(app: tauri::AppHandle) -> Result<(), String> {
    let listener =
        TcpListener::bind(BIND_ADDR).map_err(|e| format!("监听 {BIND_ADDR} 失败：{e}"))?;
    let active = Arc::new(AtomicUsize::new(0));
    bridge_log(&format!("listening on http://{BIND_ADDR}/voice"));
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let Some(permit) = VoiceConnectionPermit::acquire(&active) else {
                    // Close immediately; do not block the accept loop on a slow peer.
                    continue;
                };
                let app = app.clone();
                let _ = std::thread::Builder::new().name("p4-voice-request".into()).spawn(move || {
                    let _permit = permit;
                    handle_connection(app, stream);
                });
            }
            Err(_) => continue,
        }
    }
    Ok(())
}

fn handle_connection(app: tauri::AppHandle, mut stream: TcpStream) {
    let peer_addr = stream.peer_addr().ok();
    let peer = peer_addr.map(|addr| addr.to_string()).unwrap_or_else(|| "unknown".into());
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let _ = stream.set_write_timeout(Some(READ_TIMEOUT));
    let (status, body) = match read_request(&mut stream) {
        Ok(Some((path, _body))) if path.split('?').next() == Some("/voice/begin") => {
            let id = path.split_once('?').and_then(|(_, query)| voice_request_id(query));
            if let (Some(peer), Some(id)) = (peer_addr, id) {
                if store_voice_begin_snapshot(peer.ip(), id) {
                    bridge_log("voice/begin accepted");
                    (200, serde_json::json!({"ok": true}))
                } else {
                    (409, serde_json::json!({"ok": false, "error": "无法冻结语音目标"}))
                }
            } else {
                (400, serde_json::json!({"ok": false, "error": "缺少有效录音标识"}))
            }
        }
        // 本机诊断/自动化入口：仅 127.0.0.1 可调用，等价于 PC 端「发送」/
        // 「取消」按钮，便于验证草稿链路，不向局域网暴露。
        Ok(Some((path, _))) if path == "/voice/submit" && peer.starts_with("127.0.0.1") => {
            match submit_draft(&app) {
                Ok(value) => (200, value),
                Err(error) => (200, serde_json::json!({"ok": false, "error": error})),
            }
        }
        Ok(Some((path, _))) if path == "/voice/cancel" && peer.starts_with("127.0.0.1") => {
            let existed = cancel_draft(&app, "本机取消");
            (200, serde_json::json!({"ok": true, "hadDraft": existed}))
        }
        Ok(Some((path, wav))) if path.split('?').next() == Some("/voice") => {
            let id = path.split_once('?').and_then(|(_, query)| voice_request_id(query));
            let snapshot = match (peer_addr, id) {
                (Some(peer), Some(id)) => take_voice_begin_info(peer.ip(), id),
                _ => None,
            };
            let current = followed_agent_id();
            if snapshot.as_ref().filter(|snapshot| !current.is_empty() && snapshot.agent_id == current).is_none() {
                (409, serde_json::json!({"ok": false, "error": "录音目标缺失、过期或已变化，请重录"}))
            } else {
                let started = Instant::now();
                match transcribe(&wav) {
                    Ok(text) => {
                        bridge_log(&format!(
                            "POST /voice from {peer} bytes={} -> ok chars={} elapsed_ms={}",
                            wav.len(),
                            text.chars().count(),
                            started.elapsed().as_millis()
                        ));
                        let snapshot = snapshot.expect("validated snapshot");
                        let agent_id = snapshot.agent_id;
                        if !agent_id.is_empty() && followed_agent_id() == agent_id {
                            match create_draft(&app, &text, &agent_id, snapshot.target) {
                                Ok(_) => (200, serde_json::json!({"ok": true, "text": text, "draft": true})),
                                Err(error) => (200, serde_json::json!({"ok": false, "error": error})),
                            }
                        } else {
                            (200, serde_json::json!({
                                "ok": false,
                                "error": "跟随的 Agent 在录音期间已切换，本次语音未投递"
                            }))
                        }
                    }
                    Err(error) => {
                        bridge_log(&format!("POST /voice from {peer} -> ASR ERROR after {}ms: {error}", started.elapsed().as_millis()));
                        (200, serde_json::json!({"ok": false, "error": error}))
                    }
                }
            }
        }
        Ok(Some((_path, _body))) => (404, serde_json::json!({"ok": false, "error": "not found"})),
        Ok(None) => {
            bridge_log(&format!("non-voice request from {peer} -> 404"));
            (404, serde_json::json!({"ok": false, "error": "not found"}))
        }
        Err(error) => {
            bridge_log(&format!("POST /voice from {peer} -> BAD REQUEST {error}"));
            (400, serde_json::json!({"ok": false, "error": error}))
        }
    };
    respond(&mut stream, status, &body);
}

/// 读一个请求。返回 Ok(None) 表示不是 POST /voice（交给 404），
/// Ok(Some((path, body))) 表示拿到了完整 body。
fn read_request(stream: &mut TcpStream) -> Result<Option<(String, Vec<u8>)>, String> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 4096];
    // 1. 读到头部结束
    let header_end = loop {
        let n = stream
            .read(&mut chunk)
            .map_err(|e| format!("读取请求失败：{e}"))?;
        if n == 0 {
            return Err("连接在对头前关闭".into());
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = find_header_end(&buf) {
            break pos;
        }
        if buf.len() > 64 * 1024 {
            return Err("请求头过大".into());
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");
    let mut content_length = 0usize;
    let mut content_type = None;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("origin") {
                return Err("不接受浏览器跨站请求".into());
            }
            if name.trim().eq_ignore_ascii_case("content-type") {
                if content_type.replace(value.trim()).is_some() {
                    return Err("重复的 Content-Type".into());
                }
            }
            if name.trim().eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap_or(0);
            }
        }
    }
    if !method.eq_ignore_ascii_case("POST") || !path.starts_with("/voice") {
        return Ok(None);
    }
    if path.split('?').next() == Some("/voice")
        && !content_type.and_then(|value| value.split(';').next())
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("audio/wav")) {
        return Err("录音请求必须使用 audio/wav".into());
    }
    if content_length > MAX_BODY {
        return Err("录音过大".into());
    }
    // 2. 补齐 body（头部之后可能已经带到一部分）
    let mut body: Vec<u8> = buf[header_end + 4..].to_vec();
    while body.len() < content_length {
        let n = stream
            .read(&mut chunk)
            .map_err(|e| format!("读取录音失败：{e}"))?;
        if n == 0 {
            return Err("连接在录音中途关闭".into());
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(content_length);
    Ok(Some((path.to_string(), body)))
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

/// 固件录音格式：PCM S16LE、16 kHz、单声道的标准 44-byte WAV。拒绝其他
/// 格式而不是把字节误送给流式识别器，以免造成“录完一直 listening”的假象。
fn decode_board_pcm_wav(wav: &[u8]) -> Result<&[u8], String> {
    if wav.len() <= 44 || &wav[0..4] != b"RIFF" || &wav[8..12] != b"WAVE"
        || &wav[12..16] != b"fmt " || &wav[36..40] != b"data" {
        return Err("设备录音不是有效 WAV".into());
    }
    let read_u16 = |offset: usize| u16::from_le_bytes([wav[offset], wav[offset + 1]]);
    let read_u32 = |offset: usize| u32::from_le_bytes([wav[offset], wav[offset + 1], wav[offset + 2], wav[offset + 3]]);
    if read_u16(20) != 1 || read_u16(22) != 1 || read_u32(24) != 16_000 || read_u16(34) != 16 {
        return Err("设备录音格式应为 16 kHz 单声道 16-bit PCM".into());
    }
    let declared = read_u32(40) as usize;
    let end = 44usize.checked_add(declared).ok_or("设备录音长度无效")?;
    if end > wav.len() || declared == 0 || declared % 2 != 0 {
        return Err("设备录音数据长度无效".into());
    }
    Ok(&wav[44..end])
}

/// 按用户当前选择的识别来源转写。整段 Wi-Fi WAV 被解成 PCM 后交给与 USB
/// 完全相同的 StreamingSpeechRecognizer；local/cloud 的 provider 分流和
/// 密钥处理只存在这一处，设备端永远不接触云端凭据。
fn transcribe(wav: &[u8]) -> Result<String, String> {
    let pcm = decode_board_pcm_wav(wav)?;
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let recognizer = crate::volcengine_asr::StreamingSpeechRecognizer::start(move |event| {
        match event {
            crate::volcengine_asr::StreamingSpeechEvent::Final { text, .. } => {
                let _ = sender.send(Ok(text));
            }
            crate::volcengine_asr::StreamingSpeechEvent::Error(error) => {
                let _ = sender.send(Err(error));
            }
            _ => {}
        }
    })?;
    for chunk in pcm.chunks(640) {
        recognizer.push_pcm(chunk)?;
    }
    recognizer.finish()?;
    let timeout = recognizer.finish_wait_timeout().max(ASR_TIMEOUT);
    match receiver.recv_timeout(timeout) {
        Ok(Ok(text)) if !text.trim().is_empty() => Ok(text),
        Ok(Ok(_)) => Err("语音识别没有返回文字".into()),
        Ok(Err(error)) => Err(error),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err("语音识别超时，请缩短录音后重试".into()),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err("语音识别连接意外结束".into()),
    }
}

fn respond(stream: &mut TcpStream, status: u16, body: &serde_json::Value) {
    let payload = body.to_string();
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        409 => "Conflict",
        _ => "Internal Server Error",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        payload.len(),
        payload
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_the_board_pcm_wav_for_the_shared_recognizer() {
        let wav = crate::local_asr::pcm_to_wav(&[1, 0, 2, 0], 16_000, 1);

        assert_eq!(decode_board_pcm_wav(&wav).unwrap(), &[1, 0, 2, 0]);
    }

    #[test]
    fn connection_limit_releases_on_drop_and_rejects_excess() {
        let active = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut permits = (0..VOICE_MAX_CONNECTIONS)
            .map(|_| VoiceConnectionPermit::acquire(&active).expect("slot"))
            .collect::<Vec<_>>();
        assert!(VoiceConnectionPermit::acquire(&active).is_none());
        permits.pop();
        assert!(VoiceConnectionPermit::acquire(&active).is_some());
        drop(permits);
        assert_eq!(active.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn request_body_limit_matches_device_capture_capacity() {
        fn request(size: usize, send_body: bool) -> Result<Option<(String, Vec<u8>)>, String> {
            let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let address = listener.local_addr().unwrap();
            let writer = std::thread::spawn(move || {
                let mut client = TcpStream::connect(address).unwrap();
                write!(client, "POST /voice?sid=0123456789abcdef HTTP/1.1\r\nContent-Type: audio/wav\r\nContent-Length: {size}\r\n\r\n").unwrap();
                if send_body { client.write_all(&vec![0; size]).unwrap(); }
            });
            let (mut server, _) = listener.accept().unwrap();
            let result = read_request(&mut server);
            writer.join().unwrap();
            result
        }
        assert_eq!(request(MAX_BODY, true).unwrap().unwrap().1.len(), MAX_BODY);
        assert!(request(MAX_BODY + 1, false).is_err());
    }

    #[test]
    fn wifi_voice_requires_exact_unchanged_explicit_session() {
        for session in ["", "auto", "new", " AUTO "] {
            assert!(crate::direct_inject_agent_input_to_session("synthetic", "codex", session).is_err());
        }
        let binding = crate::P4SessionBinding { board_device_id: "p4".into(), agent_id: "codex".into(),
            session_id: "session-a".into(), generation: 7, ..Default::default() };
        let target = WifiVoiceTarget::from_binding("codex", &binding).unwrap();
        assert!(target.matches(&binding));
        let mut changed = binding.clone(); changed.session_id = "session-b".into();
        assert!(!target.matches(&changed));
        changed = binding.clone(); changed.generation += 1;
        assert!(!target.matches(&changed));
        changed = binding.clone(); changed.auto_follow = true;
        assert!(WifiVoiceTarget::from_binding("codex", &changed).is_none());
        changed = binding.clone(); changed.session_id.clear();
        assert!(WifiVoiceTarget::from_binding("codex", &changed).is_none());
        changed.session_id = "new".into();
        assert!(WifiVoiceTarget::from_binding("codex", &changed).is_none());
        changed = binding.clone(); changed.agent_id = "workbuddy".into();
        assert!(WifiVoiceTarget::from_binding("workbuddy", &changed).is_none());
        let peer: IpAddr = "192.0.2.12".parse().unwrap();
        let id = "0123456789abcdef";
        let mut store = VoiceSnapshotStore::default();
        let now = Instant::now();
        assert!(store.insert(peer, id, "codex", now));
        store.pending.get_mut(&(peer, id.into())).unwrap().target = Some(target);
        let taken = store.take_scoped(peer, id, now).unwrap();
        assert!(taken.target.unwrap().matches(&binding));
        assert!(store.take_scoped(peer, id, now).is_none());
    }

    #[test]
    fn wifi_draft_decisions_require_matching_vertical_joystick_events() {
        let event = |direction, action, board| serde_json::json!({
            "event": direction, "action": action, "boardDeviceId": board, "handledLocally": false
        });
        assert_eq!(wifi_draft_decision("input/event", &event("joystick.up", "voice_draft_send", "p4"), "p4"), Some(true));
        assert_eq!(wifi_draft_decision("input/event", &event("joystick.down", "voice_draft_cancel", "p4"), "p4"), Some(false));
        for payload in [event("button.1.short_press", "page_enter", "p4"),
            event("knob.rotate_ccw", "voice_draft_send", "p4"),
            event("joystick.up", "voice_draft_send", "other")] {
            assert_eq!(wifi_draft_decision("input/event", &payload, "p4"), None);
        }
        assert_eq!(wifi_draft_decision("input/action", &event("joystick.up", "voice_draft_send", "p4"), "p4"), None);
    }

    #[test]
    fn draft_submission_requires_current_matching_agent_and_unexpired_text() {
        let draft = VoiceDraft { text: "测试".into(), agent_id: "codex".into(),
            utterance_id: "u".into(), at_ms: 1000, target: None };
        assert!(validate_draft_submission(&draft, "codex", 1001).is_ok());
        assert!(validate_draft_submission(&draft, "", 1001).is_err());
        assert!(validate_draft_submission(&draft, "other", 1001).is_err());
        assert!(validate_draft_submission(&draft, "codex", 1001 + VOICE_DRAFT_TTL_MS).is_err());
        let empty = VoiceDraft { text: "   ".into(), ..draft };
        assert!(validate_draft_submission(&empty, "codex", 1001).is_err());
    }

    #[test]
    fn voice_snapshots_are_bounded_scoped_expiring_and_single_use() {
        let mut store = VoiceSnapshotStore::default();
        let a: std::net::IpAddr = "192.0.2.1".parse().unwrap();
        let b: std::net::IpAddr = "192.0.2.2".parse().unwrap();
        let now = std::time::Instant::now();
        assert!(store.take(a, "0000000000000001", now).is_none());
        assert!(store.insert(a, "0000000000000001", "codex", now));
        assert!(store.insert(b, "0000000000000001", "other", now));
        assert!(!store.insert(a, "0000000000000001", "other", now));
        assert!(store.take(a, "0000000000000002", now).is_none());
        assert_eq!(store.take(a, "0000000000000001", now).as_deref(), Some("codex"));
        assert!(store.take(a, "0000000000000001", now).is_none());
        assert_eq!(store.take(b, "0000000000000001", now).as_deref(), Some("other"));
        assert!(store.insert(a, "0000000000000003", "codex", now));
        assert!(store.take(a, "0000000000000003", now + Duration::from_secs(121)).is_none());
        assert!(!store.insert(a, "invalid", "codex", now));
        assert!(!store.insert(a, "0000000000000004", "", now));
        for index in 0..16 { assert!(store.insert(a, &format!("{index:016x}"), "codex", now + Duration::from_millis(index))); }
        assert!(store.insert(a, "ffffffffffffffff", "codex", now + Duration::from_millis(16)));
        assert!(store.take(a, "0000000000000000", now + Duration::from_millis(16)).is_none());
        assert_eq!(store.take(a, "ffffffffffffffff", now + Duration::from_millis(16)).as_deref(), Some("codex"));
    }

    #[test]
    fn voice_id_query_requires_one_exact_identifier() {
        assert_eq!(voice_request_id("sid=0123456789abcdef"), Some("0123456789abcdef"));
        assert_eq!(voice_request_id("x=1&sid=0123456789abcdef"), Some("0123456789abcdef"));
        assert!(voice_request_id("x=1").is_none());
        assert!(voice_request_id("sid=0123456789abcdef&sid=fedcba9876543210").is_none());
        assert!(voice_request_id("sid=0123456789abcdeF").is_none());
    }

    #[test]
    fn delayed_begin_is_matched_only_to_its_recording() {
        let peer: IpAddr = "192.0.2.123".parse().unwrap();
        let id = "abc1230000000001";
        let worker = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(60));
            assert!(voice_snapshots().lock().unwrap().insert(peer, id, "codex", Instant::now()));
        });
        assert_eq!(take_voice_begin_snapshot(peer, id).as_deref(), Some("codex"));
        worker.join().unwrap();
        assert!(voice_snapshots().lock().unwrap().take(peer, id, Instant::now()).is_none());
    }

    #[test]
    fn browser_origin_and_plain_text_cannot_reach_voice_handler() {
        use std::io::Write;
        fn request(raw: &'static [u8]) -> Result<Option<(String, Vec<u8>)>, String> {
            let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let address = listener.local_addr().unwrap();
            let writer = std::thread::spawn(move || {
                let mut client = TcpStream::connect(address).unwrap();
                client.write_all(raw).unwrap();
            });
            let (mut server, _) = listener.accept().unwrap();
            let result = read_request(&mut server);
            writer.join().unwrap();
            result
        }
        assert!(request(b"POST /voice?sid=0123456789abcdef HTTP/1.1\r\nOrigin: https://evil.example\r\nContent-Type: audio/wav\r\nContent-Length: 0\r\n\r\n").is_err());
        assert!(request(b"POST /voice?sid=0123456789abcdef HTTP/1.1\r\nContent-Type: text/plain\r\nContent-Length: 0\r\n\r\n").is_err());
        assert!(request(b"POST /voice?sid=0123456789abcdef HTTP/1.1\r\nContent-Type: audio/wav\r\nContent-Length: 0\r\n\r\n").unwrap().is_some());
    }

    #[test]
    fn header_end_detection() {
        let buf = b"POST /voice HTTP/1.1\r\nContent-Length: 3\r\n\r\nabc";
        let pos = find_header_end(buf).expect("header end");
        assert_eq!(&buf[pos + 4..], b"abc");
    }

    #[test]
    fn non_voice_path_is_rejected_as_404() {
        // 只验证解析分支：非 POST /voice 的请求行返回 Ok(None)
        for (request, expect_voice) in [
            (&b"GET /other HTTP/1.1\r\nContent-Length: 0\r\n\r\n"[..], false),
            (&b"POST /voice HTTP/1.1\r\nContent-Length: 0\r\n\r\n"[..], true),
            (&b"POST /voice?a=b HTTP/1.1\r\nContent-Length: 0\r\n\r\n"[..], true),
        ] {
            let head = String::from_utf8_lossy(request).to_string();
            let mut lines = head.split("\r\n");
            let mut parts = lines.next().unwrap().split_whitespace();
            let method = parts.next().unwrap();
            let path = parts.next().unwrap();
            let is_voice = method.eq_ignore_ascii_case("POST") && path.starts_with("/voice");
            assert_eq!(is_voice, expect_voice);
        }
    }

    #[test]
    fn content_length_is_parsed() {
        let head = "POST /voice HTTP/1.1\r\nContent-Length: 35868\r\n\r\n";
        let mut content_length = 0usize;
        for line in head.split("\r\n") {
            if let Some((name, value)) = line.split_once(':') {
                if name.trim().eq_ignore_ascii_case("content-length") {
                    content_length = value.trim().parse().unwrap_or(0);
                }
            }
        }
        assert_eq!(content_length, 35868);
    }
}
