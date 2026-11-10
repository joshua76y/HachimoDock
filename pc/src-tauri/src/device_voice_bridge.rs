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
use std::time::Duration;

const BIND_ADDR: &str = "0.0.0.0:8901";
/// 设备录音上限 60s × 16kHz × 2字节 ≈ 2MB；留足余量防滥用。
const MAX_BODY: usize = 16 * 1024 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// 本地 ASR 冷加载 + 识别的最坏情况（Qwen3 冷启动约 19s）。
const ASR_TIMEOUT: Duration = Duration::from_secs(45);

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
        Ok(Some(wav)) => match transcribe(&wav) {
            Ok(text) => {
                bridge_log(&format!(
                    "POST /voice from {peer} bytes={} -> ok text=\"{}\"",
                    wav.len(),
                    text.chars().take(60).collect::<String>()
                ));
                deliver_to_conversation(&app, &text);
                (200, serde_json::json!({"ok": true, "text": text}))
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

/// 识别出的文字必须"传送出去"：走与 USB 麦克风相同的 input/action 投递线
/// （实时对话进行中则进对话，否则进入跟随 Agent 的草稿/发送链路）。
/// 设备本地气泡由固件自己的 speech/text 负责，这里补上 PC 侧的下游。
fn deliver_to_conversation(app: &tauri::AppHandle, text: &str) {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    let payload = serde_json::json!({
        "view": "voice_input",
        "state": text,
        "type": "voice_ptt_streaming_stt",
        "event": "device.microphone.hold",
        "transport": "wifi-http",
        "voiceUtteranceId": format!("wifi-ptt-{stamp}"),
        "requestId": format!("p4-wifi-ptt-{stamp}"),
    });
    eprintln!("[device-voice-bridge] recognized text delivered via input/action: {text}");
    crate::handle_incoming_usb_message(app, "input/action".to_string(), payload);
}

/// 读一个请求。返回 Ok(None) 表示不是 POST /voice（交给 404），
/// Ok(Some(wav)) 表示拿到了完整 body。
fn read_request(stream: &mut TcpStream) -> Result<Option<Vec<u8>>, String> {
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
    Ok(Some(body))
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
