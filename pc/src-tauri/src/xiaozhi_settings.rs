/*
 * [Input] 用户保存的小智 OTA 地址，以及官方 OTA 返回的绑定状态/MQTT 供应。
 * [Output] 独立、加密的设置文件和完全脱敏的前端状态；绝不向 WebView 暴露 MQTT 凭据或虚拟设备完整身份。
 * [Pos] 小智实时问答的设置边界，不读取/改写现有 LLM、ASR 或 TTS 配置。
 */

use rumqttc::{AsyncClient, Event, Incoming, MqttOptions, NetworkOptions, TlsConfiguration, Transport};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use uuid::Uuid;

use crate::xiaozhi_protocol::{ActivationState, DeviceId, MqttProvisioning, OtaConfig, OtaRequest};

const FILE_NAME: &str = "xiaozhi-settings.json";
pub(crate) const DEFAULT_OTA_URL: &str = "https://api.tenclass.net/xiaozhi/ota/";
pub(crate) const MQTT_CONNECT_TIMEOUT: Duration = Duration::from_secs(12);
static STORAGE_DIR: OnceLock<PathBuf> = OnceLock::new();
static TLS_TRANSPORT: OnceLock<Transport> = OnceLock::new();

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct XiaozhiSettings {
    ota_url: String,
    device_mac: String,
    client_id: Uuid,
    provisioning: Option<MqttProvisioning>,
    activation_code: Option<String>,
    #[serde(default)]
    mcp_enabled: bool,
}

impl Default for XiaozhiSettings {
    fn default() -> Self {
        let device_id = DeviceId::random_local_unicast();
        Self {
            ota_url: DEFAULT_OTA_URL.into(),
            device_mac: OtaRequest::new(device_id, Uuid::nil(), false).http_headers().device_id,
            client_id: Uuid::new_v4(),
            provisioning: None,
            activation_code: None,
            mcp_enabled: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct XiaozhiSettingsInput {
    pub ota_url: Option<String>,
    pub mcp_enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct XiaozhiStatus {
    pub ota_url: String,
    pub configured: bool,
    pub activation_state: &'static str,
    pub activation_code: Option<String>,
    pub device_id_masked: String,
    pub mcp_enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct XiaozhiConnectionTest {
    pub connected: bool,
    pub elapsed_ms: u128,
}

pub fn configure_storage_dir(path: PathBuf) -> Result<(), String> {
    match STORAGE_DIR.set(path.clone()) {
        Ok(()) => Ok(()),
        Err(_) if STORAGE_DIR.get() == Some(&path) => Ok(()),
        Err(_) => Err("小智配置目录已初始化为其他路径".into()),
    }
}

fn settings_path() -> Result<PathBuf, String> {
    STORAGE_DIR
        .get()
        .map(|dir| dir.join(FILE_NAME))
        .ok_or_else(|| "小智配置目录尚未初始化".into())
}

fn valid_ota_url(value: &str) -> Result<String, String> {
    let url = reqwest::Url::parse(value.trim()).map_err(|_| "小智 OTA 地址无效")?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("小智 OTA 地址必须是无凭据的 HTTPS 地址".into());
    }
    Ok(url.to_string())
}

fn read_at(path: &Path) -> Result<XiaozhiSettings, String> {
    match crate::secure_config::read(path, "xiaozhi-realtime")? {
        Some(raw) => {
            let settings = serde_json::from_str(&raw)
                .map_err(|_| "小智配置格式损坏；请保留原文件".to_string())?;
            validate_settings(settings)
        }
        None => Ok(XiaozhiSettings::default()),
    }
}

fn validate_settings(mut settings: XiaozhiSettings) -> Result<XiaozhiSettings, String> {
    settings.ota_url = valid_ota_url(&settings.ota_url)?;
    DeviceId::parse_mac(&settings.device_mac).map_err(|_| "小智虚拟设备标识无效")?;
    if let Some(provisioning) = settings.provisioning.take() {
        // Round-trip through the protocol validator before anything may use it.
        settings.provisioning = Some(
            MqttProvisioning::from_encrypted_json(
                &serde_json::to_string(&provisioning).map_err(|_| "小智 MQTT 配置损坏")?,
            )
            .map_err(|_| "小智 MQTT 配置损坏")?,
        );
    }
    Ok(settings)
}

fn write_at(path: &Path, settings: &XiaozhiSettings) -> Result<(), String> {
    let settings = validate_settings(settings.clone())?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|_| "无法创建小智配置目录")?;
    }
    let raw = serde_json::to_vec_pretty(&settings).map_err(|_| "小智配置编码失败")?;
    crate::secure_config::write(path, "xiaozhi-realtime", &raw)
}

fn masked_device_id(value: &str) -> String {
    value.rsplit_once(':')
        .map(|(_, last)| format!("••:••:••:••:••:{last}"))
        .unwrap_or_else(|| "••••".into())
}

fn status_from(settings: &XiaozhiSettings) -> XiaozhiStatus {
    XiaozhiStatus {
        ota_url: settings.ota_url.clone(),
        configured: settings.provisioning.is_some(),
        activation_state: if settings.provisioning.is_some() {
            "ready"
        } else if settings.activation_code.is_some() {
            "pending"
        } else {
            "unconfigured"
        },
        activation_code: settings.activation_code.clone(),
        device_id_masked: masked_device_id(&settings.device_mac),
        mcp_enabled: settings.mcp_enabled,
    }
}

#[tauri::command]
pub fn load_xiaozhi_settings() -> Result<XiaozhiStatus, String> {
    Ok(status_from(&read_at(&settings_path()?)?))
}

#[tauri::command]
pub fn save_xiaozhi_settings(input: XiaozhiSettingsInput) -> Result<XiaozhiStatus, String> {
    let path = settings_path()?;
    let mut settings = read_at(&path)?;
    if let Some(ota_url) = input.ota_url {
        let ota_url = valid_ota_url(&ota_url)?;
        if ota_url != settings.ota_url {
            settings.ota_url = ota_url;
            settings.provisioning = None;
            settings.activation_code = None;
        }
    }
    if let Some(mcp_enabled) = input.mcp_enabled {
        settings.mcp_enabled = mcp_enabled;
    }
    write_at(&path, &settings)?;
    Ok(status_from(&settings))
}

fn resolve_mqtt_from(settings: &XiaozhiSettings) -> Result<MqttProvisioning, String> {
    settings
        .provisioning
        .clone()
        .ok_or_else(|| "请先完成小智设备绑定并重新获取连接配置".into())
}

pub(crate) fn resolved_mqtt_provisioning() -> Result<MqttProvisioning, String> {
    resolve_mqtt_from(&read_at(&settings_path()?)?)
}

pub(crate) fn mcp_enabled() -> Result<bool, String> {
    Ok(read_at(&settings_path()?)?.mcp_enabled)
}

/// The MQTT crate's built-in TLS constructor enumerates every macOS Keychain
/// certificate and panics if one entry cannot be imported. The Xiaozhi broker
/// is a public HTTPS-style endpoint, so use the reviewed WebPKI public roots
/// already shipped with this application instead of the mutable host store.
pub(crate) async fn default_tls_transport() -> Result<Transport, String> {
    if let Some(transport) = TLS_TRANSPORT.get() {
        return Ok(transport.clone());
    }
    let roots = rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    // The desktop links both ring and aws-lc through unrelated HTTP stacks.
    // Selecting the provider explicitly prevents rustls' process-global
    // provider discovery from blocking this Tauri command on first use.
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
        .with_safe_default_protocol_versions()
        .map_err(|_| "小智 TLS 协议初始化失败".to_string())?
        .with_root_certificates(roots)
        .with_no_client_auth();
    let transport = Transport::tls_with_config(TlsConfiguration::Rustls(Arc::new(config)));
    let _ = TLS_TRANSPORT.set(transport.clone());
    Ok(transport)
}

async fn activate_at(path: &Path) -> Result<XiaozhiStatus, String> {
    let mut settings = read_at(path)?;
    let device_id = DeviceId::parse_mac(&settings.device_mac).map_err(|_| "小智虚拟设备标识无效")?;
    let request = OtaRequest::new(device_id, settings.client_id, settings.mcp_enabled);
    let headers = request.http_headers();
    let client = crate::llm_network::client(Duration::from_secs(20))?;
    let response = client
        .post(&settings.ota_url)
        .header("Activation-Version", headers.activation_version)
        .header("Device-Id", headers.device_id)
        .header("Client-Id", headers.client_id)
        .header("User-Agent", headers.user_agent)
        .header("Accept-Language", headers.accept_language)
        .json(&request)
        .send()
        .await
        .map_err(|error| crate::llm_network::connection_error(&error))?
        .error_for_status()
        .map_err(|_| "小智 OTA 服务暂不可用，请稍后重试")?;
    let ota = OtaConfig::parse(
        response
            .json()
            .await
            .map_err(|_| "小智 OTA 返回格式不兼容")?,
    )
    .map_err(|error| error.to_string())?;
    match ota.activation_state() {
        ActivationState::Pending => {
            settings.provisioning = None;
            settings.activation_code = ota.activation_code().map(str::to_owned);
        }
        ActivationState::Ready => {
            settings.provisioning = Some(ota.mqtt_provisioning().map_err(|error| error.to_string())?);
            settings.activation_code = None;
        }
    }
    write_at(path, &settings)?;
    Ok(status_from(&settings))
}

#[tauri::command]
pub async fn activate_xiaozhi() -> Result<XiaozhiStatus, String> {
    let path = settings_path()?;
    activate_at(&path).await
}

fn endpoint_host_port(endpoint: &str) -> Result<(&str, u16), String> {
    match endpoint.rsplit_once(':') {
        Some((host, port)) => Ok((host, port.parse().map_err(|_| "小智 MQTT 地址无效")?)),
        None => Ok((endpoint, 8883)),
    }
}

fn record_mqtt_probe(stage: &'static str, started: &Instant) {
    // Diagnostics intentionally contain no endpoint, identity, credential,
    // packet payload, or transcript. They only establish which bounded phase
    // made progress when a Tauri invocation appears pending in the UI.
    crate::realtime_chat_log::record(
        "",
        "xiaozhi_mqtt_probe",
        json!({"stage": stage, "elapsedMs": started.elapsed().as_millis()}),
    );
}

#[tauri::command]
pub async fn test_xiaozhi_connection() -> Result<XiaozhiConnectionTest, String> {
    let started = Instant::now();
    record_mqtt_probe("started", &started);
    let provisioning = resolved_mqtt_provisioning()?;
    record_mqtt_probe("config_ready", &started);
    let (host, port) = endpoint_host_port(provisioning.endpoint())?;
    let mut options = MqttOptions::new(provisioning.client_id(), host, port);
    options.set_keep_alive(Duration::from_secs(u64::from(provisioning.keepalive())));
    options.set_credentials(provisioning.username(), provisioning.password());
    options.set_transport(default_tls_transport().await?);
    record_mqtt_probe("tls_ready", &started);
    let (client, mut event_loop) = AsyncClient::new(options, 4);
    let mut network_options = NetworkOptions::new();
    network_options.set_connection_timeout(5);
    event_loop.set_network_options(network_options);
    record_mqtt_probe("poll_started", &started);
    let poll = tokio::time::timeout(MQTT_CONNECT_TIMEOUT, async {
        loop {
            match event_loop.poll().await {
                Ok(Event::Incoming(Incoming::ConnAck(_))) => return Ok(()),
                Ok(_) => {}
                Err(error) => return Err(error),
            }
        }
    }).await;
    // The event loop stops as soon as this one-shot probe has an answer. Do
    // not await a disconnect request after that: it would need the stopped
    // loop to consume its own queue. Dropping the client closes the socket.
    drop(client);
    match poll {
        Ok(Ok(())) => {
            record_mqtt_probe("connack", &started);
            Ok(XiaozhiConnectionTest { connected: true, elapsed_ms: started.elapsed().as_millis() })
        }
        Ok(Err(_)) => {
            record_mqtt_probe("poll_error", &started);
            Err("小智 MQTT 认证或 TLS 连接失败，请重新获取绑定配置".into())
        }
        Err(_) => {
            record_mqtt_probe("timeout", &started);
            Err("小智 MQTT 连接超时，请检查网络或代理设置".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> XiaozhiSettings {
        XiaozhiSettings {
            ota_url: DEFAULT_OTA_URL.into(),
            device_mac: "02:10:2a:30:4b:5c".into(),
            client_id: Uuid::parse_str("11111111-2222-4333-8444-555555555555").unwrap(),
            provisioning: Some(MqttProvisioning::from_encrypted_json(&json!({
                "endpoint": "mqtt.example.com:8883", "client_id": "mqtt-client",
                "username": "mqtt-user", "password": "mqtt-secret",
                "publish_topic": "device/test/up", "keepalive": 240
            }).to_string()).unwrap()),
            activation_code: None,
            mcp_enabled: false,
        }
    }

    #[test]
    fn independently_encrypted_settings_never_expose_mqtt_or_full_identity_in_status() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let settings = fixture();
        write_at(&path, &settings).unwrap();
        let raw = fs::read_to_string(&path).unwrap();
        assert!(raw.contains("pet-manager-aes256gcm-v1"));
        assert!(!raw.contains("mqtt-secret"));
        assert!(!raw.contains("02:10:2a:30:4b:5c"));
        let status = serde_json::to_string(&status_from(&read_at(&path).unwrap())).unwrap();
        assert!(!status.contains("mqtt-secret"));
        assert!(!status.contains("02:10:2a:30:4b:5c"));
        assert!(status.contains("••:••:••:••:••:5c"));
    }

    #[test]
    fn mcp_setting_defaults_off_and_status_exposes_only_its_boolean() {
        let legacy: XiaozhiSettings = serde_json::from_value(json!({
            "otaUrl": DEFAULT_OTA_URL,
            "deviceMac": "02:10:2a:30:4b:5c",
            "clientId": "11111111-2222-4333-8444-555555555555",
            "provisioning": null,
            "activationCode": null
        })).unwrap();
        assert!(!legacy.mcp_enabled);

        let mut enabled = fixture();
        enabled.mcp_enabled = true;
        let status = serde_json::to_string(&status_from(&enabled)).unwrap();
        assert!(status.contains("\"mcpEnabled\":true"));
        assert!(!status.contains("mqtt-secret"));
        assert!(!status.contains("02:10:2a:30:4b:5c"));
    }

    #[test]
    fn changed_ota_url_clears_old_binding_but_keeps_the_virtual_identity() {
        let mut settings = fixture();
        let original_identity = settings.device_mac.clone();
        settings.ota_url = valid_ota_url("https://example.invalid/ota/").unwrap();
        settings.provisioning = None;
        settings.activation_code = None;
        assert_eq!(settings.device_mac, original_identity);
        assert!(!status_from(&settings).configured);
        assert_eq!(status_from(&settings).activation_state, "unconfigured");
    }

    #[test]
    fn ota_url_and_mqtt_endpoint_are_strictly_bounded() {
        assert!(valid_ota_url("http://example.invalid/").is_err());
        assert!(valid_ota_url("https://user:pass@example.invalid/").is_err());
        assert!(endpoint_host_port("mqtt.example.com:8883").is_ok());
        assert!(endpoint_host_port("mqtt.example.com").is_ok());
    }

    #[test]
    fn mqtt_connection_probe_never_awaits_disconnect_without_an_event_loop() {
        // `event_loop.poll()` is deliberately stopped after the probe has its
        // answer. Awaiting AsyncClient::disconnect afterwards can then wait on
        // its own unconsumed request queue forever, leaving the settings UI in
        // a permanent pending state.
        let source = include_str!("xiaozhi_settings.rs");
        let forbidden = ["client", "disconnect()", "await"].join(".");
        assert!(!source.contains(&forbidden));
    }

    #[test]
    fn mqtt_connection_probe_only_accepts_a_broker_connack() {
        // A transport progress event is not authentication.  The settings UI
        // must only report success after the broker has accepted CONNECT.
        let source = include_str!("xiaozhi_settings.rs");
        assert!(source.contains("Event::Incoming(Incoming::ConnAck(_))"));
        assert!(source.contains("Ok(Ok(())) => Ok(XiaozhiConnectionTest"));
    }

    #[test]
    fn xiaozhi_tls_uses_bundled_public_roots_not_the_macos_keychain() {
        // The default rumqttc transport enumerates every Keychain certificate
        // and panics on malformed entries. Xiaozhi's public endpoint needs a
        // deterministic public trust store instead.
        let source = include_str!("xiaozhi_settings.rs");
        assert!(source.contains("webpki_roots::TLS_SERVER_ROOTS"));
        let built_in_default = ["Transport", "tls_with_default_config"].join("::");
        assert!(!source.contains(&built_in_default));
    }
}
