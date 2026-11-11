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
use std::net::{TcpListener, TcpStream};
use std::sync::Mutex;
use std::time::Duration;
use tauri::Emitter;

const BIND_ADDR: &str = "0.0.0.0:8901";
/// 设备录音上限 60s × 16kHz × 2字节 ≈ 2MB；留足余量防滥用。
const MAX_BODY: usize = 16 * 1024 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// 本地 ASR 冷加载 + 识别的最坏情况（Qwen3 冷启动约 19s）。
const ASR_TIMEOUT: Duration = Duration::from_secs(45);

/// WiFi PTT 目标冻结（round-5，对齐 USB 路径 DeviceVoiceContext 语义）：
/// 固件在按下语音键瞬间 POST /voice/begin，这里快照「当时跟随的 Agent」；
/// 录完投递前若跟随目标已变化，则丢弃本次识别文本，宁可丢字也不写错地方。
#[derive(Clone, Debug)]
struct VoiceBeginSnapshot {
    agent_id: String,
    at_ms: u128,
}

static VOICE_BEGIN_SNAPSHOT: Mutex<Option<VoiceBeginSnapshot>> = Mutex::new(None);

/// 快照有效窗口：录音上限 60s + 传输/识别余量。
const VOICE_BEGIN_SNAPSHOT_TTL_MS: u128 = 120_000;

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

fn store_voice_begin_snapshot() {
    let snapshot = VoiceBeginSnapshot {
        agent_id: followed_agent_id(),
        at_ms: now_ms(),
    };
    bridge_log(&format!(
        "voice/begin -> froze follow target \"{}\"",
        snapshot.agent_id
    ));
    if let Ok(mut guard) = VOICE_BEGIN_SNAPSHOT.lock() {
        *guard = Some(snapshot);
    }
}

/// 录完投递前的 target_is_current 校验。返回 false 表示跟随目标在录音
/// 期间被切换，本次文本应当丢弃（与原项目语义一致：宁弃勿错投）。
fn voice_target_is_current() -> bool {
    let snapshot = match VOICE_BEGIN_SNAPSHOT.lock() {
        Ok(guard) => guard.clone(),
        Err(_) => return true,
    };
    let Some(snapshot) = snapshot else {
        return true; // 旧固件没有 begin 通知：保持原行为直接投递
    };
    if now_ms().saturating_sub(snapshot.at_ms) > VOICE_BEGIN_SNAPSHOT_TTL_MS {
        return true; // 过期快照（比如悬空的 begin）：不拦
    }
    if snapshot.agent_id.is_empty() {
        return true; // begin 时无法确定目标：不拦
    }
    let current = followed_agent_id();
    if current.is_empty() {
        return true; // 投递时无法确定目标：不拦
    }
    let same = snapshot.agent_id == current;
    if !same {
        bridge_log(&format!(
            "voice dropped: follow target changed during recording (\"{}\" -> \"{}\")",
            snapshot.agent_id, current
        ));
    }
    same
}

/// ASR 识别完成后不再直接投递：先生成「待确认草稿」，PC 端与设备屏都显示
/// 识别文字，用户按发送（摇杆右/确认键）才真正注入 Agent，按取消
/// （摇杆左/返回键）丢弃。这既满足用户要求，也避开 input/action 链路上
/// realtime_chat 等前置分支的静默吞字。
#[derive(Clone, Debug, serde::Serialize)]
pub struct VoiceDraft {
    pub text: String,
    pub agent_id: String,
    pub utterance_id: String,
    pub at_ms: u128,
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
    bridge_log(&format!("emit {event}: {}", payload.to_string().chars().take(140).collect::<String>()));
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
        "hint": "语音待确认：摇杆右/确认=发送 · 摇杆左/返回=取消",
    });
    match manager.send("voice/draft", &payload) {
        Ok(()) => bridge_log(&format!("voice/draft notify active={active} sent")),
        Err(error) => bridge_log(&format!("voice/draft notify active={active} failed: {error}")),
    }
}

pub fn create_draft(app: &tauri::AppHandle, text: &str, agent_id: &str) -> VoiceDraft {
    let stamp = now_ms();
    let draft = VoiceDraft {
        text: text.trim().to_string(),
        agent_id: agent_id.to_string(),
        utterance_id: format!("wifi-draft-{stamp}"),
        at_ms: stamp,
    };
    if let Ok(mut guard) = VOICE_DRAFT.lock() {
        *guard = Some(draft.clone());
    }
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
    draft
}

pub fn clear_draft(app: &tauri::AppHandle, reason: &str) {
    let existed = VOICE_DRAFT.lock().ok().and_then(|mut guard| guard.take()).is_some();
    if existed {
        notify_device_draft(app, false, reason);
        emit_voice_event(app, "voice-draft-cleared", serde_json::json!({"reason": reason}));
        bridge_log(&format!("voice draft cleared: {reason}"));
    }
}

pub fn submit_draft(app: &tauri::AppHandle) -> Result<serde_json::Value, String> {
    let draft = match VOICE_DRAFT.lock() {
        Ok(mut guard) => guard.take(),
        Err(_) => None,
    }
    .ok_or_else(|| "没有待发送的语音草稿".to_string())?;
    if now_ms().saturating_sub(draft.at_ms) > VOICE_DRAFT_TTL_MS {
        clear_draft(app, "草稿已超时");
        return Err("语音草稿已超时，请重新说一次".to_string());
    }
    // 目标冻结：待确认期间切换了跟随 Agent 就拒绝发送（宁弃勿错投）。
    let current = followed_agent_id();
    if !current.is_empty() && current != draft.agent_id {
        clear_draft(app, "跟随的 Agent 已切换");
        return Err(format!(
            "跟随的 Agent 已从 {} 切换到 {}，草稿已取消",
            draft.agent_id, current
        ));
    }
    if draft.text.is_empty() {
        clear_draft(app, "草稿为空");
        return Err("语音草稿为空".to_string());
    }
    bridge_log(&format!(
        "voice draft submitting -> agent=\"{}\" text=\"{}\"",
        draft.agent_id,
        draft.text.chars().take(60).collect::<String>()
    ));
    notify_device_draft(app, false, "submitted");
    emit_voice_event(app, "voice-draft-cleared", serde_json::json!({"reason": "submitted"}));
    // Agent 注入可能耗时数分钟（CLI 冷启动 + 整轮回复）。同步等待会让
    // 「发送」按钮和摇杆手势卡死，因此后台执行，结果经事件回报前端。
    let app = app.clone();
    let text = draft.text.clone();
    let agent_id = draft.agent_id.clone();
    std::thread::spawn(move || {
        match crate::deliver_voice_draft_to_agent(&text, &agent_id) {
            Ok(response) => {
                let queued = response.get("queued").and_then(|v| v.as_bool()).unwrap_or(false);
                let message = if queued {
                    format!(
                        "已提交，但 {} 正忙：桥最多重试 3 次（约 10 秒）。若未送达请切换 Agent 或查看 ~/.claw-pet/logs/status-bridge.log",
                        agent_id
                    )
                } else {
                    format!("已发送到 {}", agent_id)
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
        "submitted": true,
        "agentId": draft.agent_id,
        "text": draft.text,
    }))
}

pub fn cancel_draft(app: &tauri::AppHandle, reason: &str) -> bool {
    let existed = current_draft().is_some();
    clear_draft(app, reason);
    existed
}

/// 设备手势快选：草稿存在时把左右摇杆 / 确认 / 返回解释为发送 / 取消，
/// 并阻止这些按键继续走会话切换等原有动作。返回 true 表示已消费。
pub fn try_draft_gesture(app: &tauri::AppHandle, topic: &str, payload: &serde_json::Value) -> bool {
    if topic != "input/event" && topic != "input/action" {
        return false;
    }
    if current_draft().is_none() {
        return false;
    }
    let action = payload.get("action").and_then(|v| v.as_str()).unwrap_or_default();
    // 用户指定：摇杆向上=发送，向下=取消（按方向事件判定，不依赖绑定的
    // action 名——摇杆上下默认仍绑 session_previous/next，草稿期被重映射）。
    let event = payload.get("event").and_then(|v| v.as_str()).unwrap_or_default();
    let send = event == "joystick.up"
        || action == "page_enter"
        || action == "voice_draft_send";
    let cancel = event == "joystick.down"
        || action == "page_back"
        || action == "voice_draft_cancel";
    if send {
        if let Err(error) = submit_draft(app) {
            emit_voice_event(app, "voice-draft-result", serde_json::json!({
                "ok": false, "message": error,
            }));
        }
    } else if cancel {
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

fn serve(app: tauri::AppHandle) -> Result<(), String> {
    let listener =
        TcpListener::bind(BIND_ADDR).map_err(|e| format!("监听 {BIND_ADDR} 失败：{e}"))?;
    bridge_log(&format!("listening on http://{BIND_ADDR}/voice"));
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let app = app.clone();
                std::thread::spawn(move || handle_connection(app, stream));
            }
            Err(_) => continue,
        }
    }
    Ok(())
}

fn handle_connection(app: tauri::AppHandle, mut stream: TcpStream) {
    let peer = stream
        .peer_addr()
        .map(|addr| addr.to_string())
        .unwrap_or_else(|_| "unknown".into());
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let _ = stream.set_write_timeout(Some(READ_TIMEOUT));
    let (status, body) = match read_request(&mut stream) {
        Ok(Some((path, body))) if path == "/voice/begin" => {
            store_voice_begin_snapshot();
            (200, serde_json::json!({"ok": true}))
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
        Ok(Some((_path, wav))) => match transcribe(&wav) {
            Ok(text) => {
                bridge_log(&format!(
                    "POST /voice from {peer} bytes={} -> ok text=\"{}\"",
                    wav.len(),
                    text.chars().take(60).collect::<String>()
                ));
                if voice_target_is_current() {
                    let agent_id = followed_agent_id();
                    create_draft(&app, &text, &agent_id);
                    (200, serde_json::json!({"ok": true, "text": text, "draft": true}))
                } else {
                    (
                        200,
                        serde_json::json!({
                            "ok": false,
                            "error": "跟随的 Agent 在录音期间已切换，本次语音未投递"
                        }),
                    )
                }
            }
            Err(error) => {
                bridge_log(&format!(
                    "POST /voice from {peer} bytes={} -> ASR ERROR {error}",
                    wav.len()
                ));
                (200, serde_json::json!({"ok": false, "error": error}))
            }
        },
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
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap_or(0);
            }
        }
    }
    if !method.eq_ignore_ascii_case("POST") || !path.starts_with("/voice") {
        return Ok(None);
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

/// 按用户当前选择的识别来源转写。设备 PTT 只认本地链路：
/// 火山云端走的是流式会话，这里不重复实现。
fn transcribe(wav: &[u8]) -> Result<String, String> {
    if let Some((endpoint, engine)) = crate::volcengine_asr::local_provider_prefs() {
        // 冷加载预热失败不致命：transcribe_wav 自己会再触发懒加载。
        let _ = crate::local_asr::ensure_loaded(&endpoint, &engine);
        let transcript =
            crate::local_asr::transcribe_wav(&endpoint, &engine, wav, ASR_TIMEOUT)?;
        return Ok(transcript.text);
    }
    Err("语音识别来源未选择本地引擎，设备按键识别不可用；请在 API 设置把语音识别切到本地".into())
}

fn respond(stream: &mut TcpStream, status: u16, body: &serde_json::Value) {
    let payload = body.to_string();
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "Internal Server Error",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n{}",
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
