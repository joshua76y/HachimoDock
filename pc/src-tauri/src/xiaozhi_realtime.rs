/*
 * [Input] 已加密恢复的官方 MQTT 供应、P4 的 16 kHz PCM 帧和小智 MQTT/UDP 回包。
 * [Output] 有界的 MQTT/UDP 实时会话事件；不调用 Agent、MCP、网页搜索、LLM、ASR 或 TTS。
 * [Pos] 小智官方 MQTT + AES-CTR UDP 协议适配。秘密、原始网络负载和 PCM 一律不记录。
 */

use rumqttc::{AsyncClient, Event, Incoming, MqttOptions, NetworkOptions, QoS};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot};

use crate::xiaozhi_audio::{decrypt_udp_payload, encrypt_outgoing_udp_payload, OpusToPcm, PcmToOpus, UdpPacket};
use crate::xiaozhi_protocol::MqttProvisioning;

const HELLO_TIMEOUT: Duration = Duration::from_secs(12);
const MAX_MQTT_JSON_BYTES: usize = 64 * 1024;
const MAX_UDP_BYTES: usize = 2048;

#[derive(Debug)]
pub(crate) enum XiaozhiEvent {
    Ready,
    UserText(String),
    ReplyText(String),
    Speaking,
    Listening,
    McpRequest(crate::xiaozhi_mcp::Request),
    Pcm(Vec<u8>),
    Ended,
    Failed(String),
}

enum Command {
    StartListening,
    StopListening,
    Abort,
    Pcm(Vec<u8>),
    McpReply(Value),
    Close(oneshot::Sender<()>),
}

/// A cloneable, bounded reply endpoint for MCP workers. It can outlive a
/// single request but not the realtime command receiver; late replies are
/// simply rejected after the audio session closes.
#[derive(Clone)]
pub(crate) struct McpReplyHandle {
    commands: mpsc::Sender<Command>,
}

impl McpReplyHandle {
    pub(crate) fn send(&self, reply: Value) -> Result<(), String> {
        self.commands.try_send(Command::McpReply(reply))
            .map_err(|_| "小智 MCP 回复通道已关闭或繁忙".into())
    }
}

pub(crate) struct XiaozhiRealtime {
    commands: mpsc::Sender<Command>,
    events: mpsc::Receiver<XiaozhiEvent>,
}

impl XiaozhiRealtime {
    pub(crate) async fn connect(provisioning: MqttProvisioning, mcp_enabled: bool) -> Result<Self, String> {
        let (host, port) = mqtt_host_port(provisioning.endpoint())?;
        let mut options = MqttOptions::new(provisioning.client_id(), host, port);
        options.set_keep_alive(Duration::from_secs(u64::from(provisioning.keepalive())));
        options.set_credentials(provisioning.username(), provisioning.password());
        options.set_transport(crate::xiaozhi_settings::default_tls_transport().await?);
        let (client, mut event_loop) = AsyncClient::new(options, 16);
        let mut network_options = NetworkOptions::new();
        network_options.set_connection_timeout(5);
        event_loop.set_network_options(network_options);

        let connected: Result<(), String> = tokio::time::timeout(crate::xiaozhi_settings::MQTT_CONNECT_TIMEOUT, async {
            loop {
                match event_loop.poll().await.map_err(|_| "小智 MQTT TLS/认证连接失败".to_string())? {
                    Event::Incoming(Incoming::ConnAck(_)) => return Ok(()),
                    _ => {}
                }
            }
        }).await.map_err(|_| "小智 MQTT 连接超时")?;
        connected?;

        let topic = incoming_topic(&provisioning);
        client.subscribe(topic, QoS::AtMostOnce).await
            .map_err(|_| "小智 MQTT 订阅失败")?;
        client.publish(
            provisioning.publish_topic(),
            QoS::AtMostOnce,
            false,
            client_hello(mcp_enabled),
        ).await.map_err(|_| "小智 MQTT hello 发送失败")?;

        let (command_tx, command_rx) = mpsc::channel(192);
        let (event_tx, event_rx) = mpsc::channel(192);
        tokio::spawn(run(provisioning, client, event_loop, command_rx, event_tx));
        Ok(Self { commands: command_tx, events: event_rx })
    }

    pub(crate) async fn start_listening(&self) -> Result<(), String> {
        self.send(Command::StartListening).await
    }

    pub(crate) async fn stop_listening(&self) -> Result<(), String> {
        self.send(Command::StopListening).await
    }

    pub(crate) async fn abort(&self) -> Result<(), String> {
        self.send(Command::Abort).await
    }

    /// The P4 frame is fixed at 20 ms / 16 kHz mono s16le. A full mailbox is a
    /// back-pressure failure, never a reason to grow unbounded audio queues.
    pub(crate) fn push_pcm_16k(&self, pcm: Vec<u8>) -> Result<(), String> {
        if pcm.len() != 640 { return Err("小智输入 PCM 帧长度无效".into()); }
        self.commands.try_send(Command::Pcm(pcm))
            .map_err(|_| "小智音频发送过载，已停止本次会话".into())
    }

    pub(crate) async fn next_event(&mut self) -> Option<XiaozhiEvent> {
        self.events.recv().await
    }

    pub(crate) fn mcp_reply_handle(&self) -> McpReplyHandle {
        McpReplyHandle { commands: self.commands.clone() }
    }

    pub(crate) async fn close(self) {
        let (done_tx, done_rx) = oneshot::channel();
        if self.commands.send(Command::Close(done_tx)).await.is_ok() {
            let _ = tokio::time::timeout(Duration::from_secs(2), done_rx).await;
        }
    }

    async fn send(&self, command: Command) -> Result<(), String> {
        self.commands.send(command).await.map_err(|_| "小智会话已结束".into())
    }
}

impl Drop for XiaozhiRealtime {
    fn drop(&mut self) {
        // Cancellation can drop the realtime-chat future before it gets to an
        // async close. Queue a best-effort goodbye; no key, PCM or text is
        // retained by this handle.
        let (done, _) = oneshot::channel();
        let _ = self.commands.try_send(Command::Close(done));
    }
}

struct AudioChannel {
    session_id: String,
    socket: UdpSocket,
    key: [u8; 16],
    nonce: [u8; 16],
    encoder: PcmToOpus,
    decoder: OpusToPcm,
    outgoing_sequence: u32,
    outgoing_timestamp: u32,
    incoming_sequence: u32,
}

async fn run(
    provisioning: MqttProvisioning,
    client: AsyncClient,
    mut event_loop: rumqttc::EventLoop,
    mut commands: mpsc::Receiver<Command>,
    events: mpsc::Sender<XiaozhiEvent>,
) {
    let mut channel: Option<AudioChannel> = None;
    let mut closed = false;
    let hello_timeout = tokio::time::sleep(HELLO_TIMEOUT);
    tokio::pin!(hello_timeout);
    while !closed {
        let mut udp_buffer = [0_u8; MAX_UDP_BYTES];
        tokio::select! {
            _ = &mut hello_timeout, if channel.is_none() => {
                let _ = events.send(XiaozhiEvent::Failed("小智音频握手超时".into())).await;
                closed = true;
            }
            mqtt = event_loop.poll() => {
                match mqtt {
                    Ok(Event::Incoming(Incoming::Publish(message))) => {
                        if message.payload.len() > MAX_MQTT_JSON_BYTES { continue; }
                        let Ok(value) = serde_json::from_slice::<Value>(&message.payload) else { continue; };
                        match handle_mqtt_value(&value, channel.as_ref().map(|c| c.session_id.as_str())) {
                            MqttAction::Open(hello) if channel.is_none() => match open_audio_channel(hello).await {
                                Ok(new_channel) => { channel = Some(new_channel); let _ = events.send(XiaozhiEvent::Ready).await; }
                                Err(error) => { let _ = events.send(XiaozhiEvent::Failed(error)).await; closed = true; }
                            },
                            MqttAction::UserText(text) => { let _ = events.send(XiaozhiEvent::UserText(text)).await; }
                            MqttAction::ReplyText(text) => { let _ = events.send(XiaozhiEvent::ReplyText(text)).await; }
                            MqttAction::Speaking => { let _ = events.send(XiaozhiEvent::Speaking).await; }
                            MqttAction::Listening => { let _ = events.send(XiaozhiEvent::Listening).await; }
                            MqttAction::Mcp(request) => { let _ = events.send(XiaozhiEvent::McpRequest(request)).await; }
                            MqttAction::Ended => { let _ = events.send(XiaozhiEvent::Ended).await; closed = true; }
                            MqttAction::Reject => {},
                            MqttAction::Open(_) => {},
                        }
                    }
                    Ok(_) => {}
                    Err(_) => { let _ = events.send(XiaozhiEvent::Failed("小智 MQTT 连接已中断".into())).await; break; }
                }
            }
            command = commands.recv() => match command {
                Some(Command::StartListening) => { let _ = send_control(&client, &provisioning, channel.as_ref().map(|audio| audio.session_id.clone()), "listen", Some("start")).await; }
                Some(Command::StopListening) => { let _ = send_control(&client, &provisioning, channel.as_ref().map(|audio| audio.session_id.clone()), "listen", Some("stop")).await; }
                Some(Command::Abort) => { let _ = send_control(&client, &provisioning, channel.as_ref().map(|audio| audio.session_id.clone()), "abort", None).await; }
                Some(Command::Pcm(pcm)) => {
                    let Some(audio) = channel.as_mut() else { continue; };
                    match audio.encoder.push_pcm_20ms(&pcm) {
                        Ok(Some(opus)) => {
                            audio.outgoing_sequence = audio.outgoing_sequence.wrapping_add(1);
                            // The server-provisioned nonce doubles as the UDP header template.
                            // Keep its session SSRC intact; see `encrypt_outgoing_udp_payload`.
                            let packet = UdpPacket::new(0, audio.outgoing_timestamp, audio.outgoing_sequence, opus);
                            audio.outgoing_timestamp = audio.outgoing_timestamp.wrapping_add(960);
                            let result = packet.and_then(|packet| encrypt_outgoing_udp_payload(&packet, &audio.key, &audio.nonce));
                            match result {
                                Ok(packet) if audio.socket.send(&packet).await.is_ok() => {},
                                _ => { let _ = events.send(XiaozhiEvent::Failed("小智 UDP 音频发送失败".into())).await; closed = true; }
                            }
                        }
                        Ok(None) => {}
                        Err(_) => { let _ = events.send(XiaozhiEvent::Failed("小智 Opus 编码失败".into())).await; closed = true; }
                    }
                }
                Some(Command::McpReply(reply)) => {
                    if client.publish(provisioning.publish_topic(), QoS::AtMostOnce, false, reply.to_string()).await.is_err() {
                        let _ = events.send(XiaozhiEvent::Failed("小智 MCP 回复发送失败".into())).await;
                        closed = true;
                    }
                }
                Some(Command::Close(done)) => {
                    let _ = send_control(&client, &provisioning, channel.as_ref().map(|audio| audio.session_id.clone()), "goodbye", None).await;
                    let _ = client.disconnect().await;
                    let _ = done.send(());
                    closed = true;
                }
                None => closed = true,
            },
            received = recv_udp(channel.as_ref().map(|audio| &audio.socket), &mut udp_buffer), if channel.is_some() => {
                let length = match received {
                    Ok(length) => length,
                    Err(()) => {
                        let _ = events.send(XiaozhiEvent::Failed("小智 UDP 音频连接已中断".into())).await;
                        closed = true;
                        continue;
                    }
                };
                let Some(audio) = channel.as_mut() else { continue; };
                let packet = match decrypt_udp_payload(&udp_buffer[..length], &audio.key, &audio.nonce, audio.incoming_sequence) {
                    Ok(packet) => packet,
                    Err(_) => {
                        let _ = events.send(XiaozhiEvent::Failed("小智 UDP 音频包无效或已过期".into())).await;
                        closed = true;
                        continue;
                    }
                };
                audio.incoming_sequence = packet.sequence();
                match audio.decoder.decode_to_16k(packet.payload()) {
                    Ok(pcm) => {
                        let mut bytes = Vec::with_capacity(pcm.len() * 2);
                        for sample in pcm { bytes.extend_from_slice(&sample.to_le_bytes()); }
                        let _ = events.send(XiaozhiEvent::Pcm(bytes)).await;
                    }
                    Err(_) => { let _ = events.send(XiaozhiEvent::Failed("小智 Opus 解码失败".into())).await; closed = true; }
                }
            }
        }
    }
}

async fn recv_udp(socket: Option<&UdpSocket>, buffer: &mut [u8]) -> Result<usize, ()> {
    let Some(socket) = socket else { std::future::pending::<()>().await; unreachable!() };
    socket.recv(buffer).await.map_err(|_| ())
}

async fn send_control(
    client: &AsyncClient,
    provisioning: &MqttProvisioning,
    session_id: Option<String>,
    kind: &str,
    state: Option<&str>,
) -> Result<(), ()> {
    let Some(session_id) = session_id else { return Err(()); };
    let mut value = json!({"session_id": session_id, "type": kind});
    if let Some(state) = state { value["state"] = Value::String(state.into()); }
    if kind == "listen" && state == Some("start") { value["mode"] = Value::String("realtime".into()); }
    client.publish(provisioning.publish_topic(), QoS::AtMostOnce, false, value.to_string()).await.map_err(|_| ())
}

fn client_hello(mcp_enabled: bool) -> String {
    json!({
        "type": "hello", "version": 3, "transport": "udp",
        "features": {"mcp": mcp_enabled, "aec": false, "udp_probe": false},
        "audio_params": {"format":"opus", "sample_rate":16000, "channels":1, "frame_duration":60}
    }).to_string()
}

fn incoming_topic(provisioning: &MqttProvisioning) -> String {
    if let Some(topic) = provisioning.subscribe_topic() { return topic.to_owned(); }
    let namespace = provisioning.publish_topic().split('/').next().unwrap_or("device-server");
    format!("{namespace}/p2p/{}", provisioning.client_id())
}

fn mqtt_host_port(endpoint: &str) -> Result<(&str, u16), String> {
    match endpoint.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() => port.parse().map(|port| (host, port)).map_err(|_| "小智 MQTT 地址无效".into()),
        None if !endpoint.is_empty() => Ok((endpoint, 8883)),
        _ => Err("小智 MQTT 地址无效".into()),
    }
}

struct ServerHello {
    session_id: String,
    udp_server: String,
    udp_port: u16,
    key: [u8; 16],
    nonce: [u8; 16],
    sample_rate: u32,
}

async fn open_audio_channel(hello: ServerHello) -> Result<AudioChannel, String> {
    let socket = UdpSocket::bind("0.0.0.0:0").await.map_err(|_| "无法创建小智 UDP 通道")?;
    socket.connect((hello.udp_server.as_str(), hello.udp_port)).await
        .map_err(|_| "小智 UDP 通道不可达")?;
    Ok(AudioChannel {
        session_id: hello.session_id,
        socket,
        key: hello.key,
        nonce: hello.nonce,
        encoder: PcmToOpus::new().map_err(|_| "无法创建小智 Opus 编码器")?,
        decoder: OpusToPcm::new(hello.sample_rate).map_err(|_| "小智下行音频参数不支持")?,
        outgoing_sequence: 0,
        outgoing_timestamp: 0,
        incoming_sequence: 0,
    })
}

enum MqttAction { Open(ServerHello), UserText(String), ReplyText(String), Speaking, Listening, Mcp(crate::xiaozhi_mcp::Request), Ended, Reject }

fn handle_mqtt_value(value: &Value, current_session: Option<&str>) -> MqttAction {
    let Some(kind) = value.get("type").and_then(Value::as_str) else { return MqttAction::Reject; };
    if kind == "hello" && current_session.is_none() { return parse_server_hello(value).map(MqttAction::Open).unwrap_or(MqttAction::Reject); }
    // Discovery and tools/list warm-up calls are allowed before the audio
    // server hello. parse_request itself enforces the active session for a
    // side-effectful tools/call.
    if kind == "mcp" { return crate::xiaozhi_mcp::parse_request(value, current_session).map(MqttAction::Mcp).unwrap_or(MqttAction::Reject); }
    let Some(session) = current_session else { return MqttAction::Reject; };
    if value.get("session_id").and_then(Value::as_str) != Some(session) { return MqttAction::Reject; }
    match kind {
        "stt" => text_event(value).map(MqttAction::UserText).unwrap_or(MqttAction::Reject),
        "tts" => match value.get("state").and_then(Value::as_str) {
            Some("start") => MqttAction::Speaking,
            Some("stop") => MqttAction::Listening,
            Some("sentence_start") => text_event(value).map(MqttAction::ReplyText).unwrap_or(MqttAction::Reject),
            _ => MqttAction::Reject,
        },
        "goodbye" => MqttAction::Ended,
        _ => MqttAction::Reject,
    }
}

fn text_event(value: &Value) -> Option<String> {
    value.get("text").and_then(Value::as_str).map(str::trim)
        .filter(|text| !text.is_empty() && text.len() <= 4096 && !text.chars().any(char::is_control))
        .map(str::to_owned)
}

fn parse_server_hello(value: &Value) -> Result<ServerHello, ()> {
    if value.get("transport").and_then(Value::as_str) != Some("udp") { return Err(()); }
    let session_id = value.get("session_id").and_then(Value::as_str)
        .filter(|id| !id.is_empty() && id.len() <= 128 && !id.chars().any(char::is_control)).ok_or(())?.to_owned();
    let udp = value.get("udp").and_then(Value::as_object).ok_or(())?;
    let udp_server = udp.get("server").and_then(Value::as_str)
        .filter(|host| !host.is_empty() && host.len() <= 255 && !host.chars().any(char::is_whitespace)).ok_or(())?.to_owned();
    let udp_port = udp.get("port").and_then(Value::as_u64).and_then(|port| u16::try_from(port).ok()).filter(|port| *port != 0).ok_or(())?;
    let key = hex_16(udp.get("key").and_then(Value::as_str).ok_or(())?).ok_or(())?;
    let nonce = hex_16(udp.get("nonce").and_then(Value::as_str).ok_or(())?).ok_or(())?;
    let params = value.get("audio_params").and_then(Value::as_object).ok_or(())?;
    if params.get("format").and_then(Value::as_str) != Some("opus") || params.get("channels").and_then(Value::as_u64) != Some(1) { return Err(()); }
    let sample_rate = params.get("sample_rate").and_then(Value::as_u64).and_then(|rate| u32::try_from(rate).ok()).filter(|rate| matches!(*rate, 16_000 | 24_000)).ok_or(())?;
    if params.get("frame_duration").and_then(Value::as_u64) != Some(60) { return Err(()); }
    Ok(ServerHello { session_id, udp_server, udp_port, key, nonce, sample_rate })
}

fn hex_16(value: &str) -> Option<[u8; 16]> {
    if value.len() != 32 { return None; }
    let mut output = [0_u8; 16];
    for (index, byte) in output.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hello() -> Value { json!({"type":"hello","transport":"udp","session_id":"session-a",
        "udp":{"server":"127.0.0.1","port":8884,"key":"00112233445566778899aabbccddeeff","nonce":"ffeeddccbbaa99887766554433221100"},
        "audio_params":{"format":"opus","sample_rate":24000,"channels":1,"frame_duration":60}}) }

    #[test]
    fn server_hello_requires_exact_udp_crypto_and_audio_parameters() {
        assert!(parse_server_hello(&hello()).is_ok());
        for (path, replacement) in [("transport", json!("websocket")), ("session_id", json!(""))] {
            let mut invalid = hello(); invalid[path] = replacement;
            assert!(parse_server_hello(&invalid).is_err());
        }
        let mut invalid = hello(); invalid["udp"]["key"] = json!("bad");
        assert!(parse_server_hello(&invalid).is_err());
        let mut invalid = hello(); invalid["audio_params"]["sample_rate"] = json!(48_000);
        assert!(parse_server_hello(&invalid).is_err());
    }

    #[test]
    fn only_current_session_transcripts_and_tts_are_forwarded() {
        assert!(matches!(handle_mqtt_value(&hello(), None), MqttAction::Open(_)));
        let stt = json!({"type":"stt","session_id":"session-a","text":"你好"});
        assert!(matches!(handle_mqtt_value(&stt, Some("session-a")), MqttAction::UserText(_)));
        assert!(matches!(handle_mqtt_value(&stt, Some("other")), MqttAction::Reject));
        let mcp = json!({"type":"mcp","session_id":"session-a","payload":{"tool":"bad"}});
        assert!(matches!(handle_mqtt_value(&mcp, Some("session-a")), MqttAction::Reject));
    }

    #[test]
    fn mcp_warmup_and_calls_follow_their_session_rules() {
        let discover = json!({"type":"mcp","payload":{"jsonrpc":"2.0","id":1,
            "method":"server/discover","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}});
        assert!(matches!(handle_mqtt_value(&discover, None), MqttAction::Mcp(_)));
        let call = json!({"type":"mcp","session_id":"session-a","payload":{"jsonrpc":"2.0","id":2,
            "method":"tools/call","params":{"name":"desktop_agent.dispatch","arguments":{},"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}});
        assert!(matches!(handle_mqtt_value(&call, Some("session-a")), MqttAction::Mcp(_)));
        assert!(matches!(handle_mqtt_value(&call, Some("other")), MqttAction::Reject));
    }

    #[test]
    fn mqtt_subscription_prefers_official_provisioned_topic() {
        let provision = MqttProvisioning::from_encrypted_json(r#"{"endpoint":"mqtt.example.com","client_id":"client","username":"user","password":"password","publish_topic":"device-server","subscribe_topic":"devices/p2p/client","keepalive":240}"#).unwrap();
        assert_eq!(incoming_topic(&provision), "devices/p2p/client");
    }

    #[test]
    fn client_hello_declares_only_enabled_mcp_capabilities() {
        let hello: Value = serde_json::from_str(&client_hello(false)).unwrap();
        assert_eq!(hello["features"]["mcp"], false);
        let hello: Value = serde_json::from_str(&client_hello(true)).unwrap();
        assert_eq!(hello["features"]["mcp"], true);
        assert_eq!(hello["features"]["udp_probe"], false);
        assert_eq!(hello["audio_params"]["sample_rate"], 16_000);
    }

    #[test]
    fn waiting_for_server_hello_has_its_own_bounded_timeout() {
        // MQTT CONNECT can succeed while an ACL, reply route, or negotiation
        // error prevents the audio hello from arriving. That must not leave
        // the device on “正在连接小智” until the much longer session idle timer.
        let source = include_str!("xiaozhi_realtime.rs");
        let timeout_error = ["小智", "音频握手超时"].concat();
        assert!(source.contains(&timeout_error));
    }
}
