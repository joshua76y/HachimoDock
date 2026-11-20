use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::error::Error;
use std::fmt;
use uuid::Uuid;

/// 虚拟小智设备的本地标识。仅允许本地管理、单播 MAC，避免冒用真实硬件地址。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct DeviceId([u8; 6]);

impl DeviceId {
    pub(crate) fn from_octets(octets: [u8; 6]) -> Result<Self, XiaozhiProtocolError> {
        let first = octets[0];
        if first & 0x01 != 0 || first & 0x02 == 0 {
            return Err(XiaozhiProtocolError::new(
                "小智虚拟设备标识必须是本地管理的单播 MAC",
            ));
        }
        Ok(Self(octets))
    }

    pub(crate) fn random_local_unicast() -> Self {
        let mut octets = [0_u8; 6];
        let mut rng = rand::rngs::OsRng;
        rng.fill_bytes(&mut octets);
        octets[0] = (octets[0] & 0xfc) | 0x02;
        // The bit operation above constructs a local unicast MAC.
        Self(octets)
    }

    pub(crate) fn parse_mac(value: &str) -> Result<Self, XiaozhiProtocolError> {
        let mut octets = [0_u8; 6];
        let mut parts = value.split(':');
        for octet in &mut octets {
            let part = parts
                .next()
                .filter(|part| part.len() == 2)
                .ok_or_else(|| XiaozhiProtocolError::new("小智虚拟设备标识无效"))?;
            *octet = u8::from_str_radix(part, 16)
                .map_err(|_| XiaozhiProtocolError::new("小智虚拟设备标识无效"))?;
        }
        if parts.next().is_some() {
            return Err(XiaozhiProtocolError::new("小智虚拟设备标识无效"));
        }
        Self::from_octets(octets)
    }

    fn mac_address(self) -> String {
        self.0
            .iter()
            .map(|octet| format!("{octet:02x}"))
            .collect::<Vec<_>>()
            .join(":")
    }
}

impl fmt::Debug for DeviceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DeviceId(<redacted>)")
    }
}

#[derive(Serialize)]
pub(crate) struct OtaRequest {
    #[serde(skip)]
    device_id: DeviceId,
    #[serde(skip)]
    client_id: Uuid,
    version: u8,
    language: &'static str,
    mac_address: String,
    uuid: String,
    protocol: OtaProtocol,
}

#[derive(Serialize)]
struct OtaProtocol {
    features: OtaFeatures,
}

#[derive(Serialize)]
struct OtaFeatures {
    mcp: bool,
    aec: bool,
}

impl OtaRequest {
    pub(crate) fn new(device_id: DeviceId, client_id: Uuid, mcp_enabled: bool) -> Self {
        Self {
            device_id,
            client_id,
            version: 2,
            language: "zh-CN",
            mac_address: device_id.mac_address(),
            uuid: client_id.to_string(),
            protocol: OtaProtocol {
                features: OtaFeatures {
                    mcp: mcp_enabled,
                    aec: false,
                },
            },
        }
    }

    pub(crate) fn http_headers(&self) -> OtaHttpHeaders {
        OtaHttpHeaders {
            activation_version: "1",
            device_id: self.device_id.mac_address(),
            client_id: self.client_id.to_string(),
            user_agent: "HachimoDock/1.0",
            accept_language: "zh-CN",
        }
    }
}

/// OTA 身份请求头。该类型只在 Rust 后端使用，不能序列化或传到前端。
pub(crate) struct OtaHttpHeaders {
    pub(crate) activation_version: &'static str,
    pub(crate) device_id: String,
    pub(crate) client_id: String,
    pub(crate) user_agent: &'static str,
    pub(crate) accept_language: &'static str,
}

impl fmt::Debug for OtaHttpHeaders {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OtaHttpHeaders")
            .field("activation_version", &self.activation_version)
            .field("device_id", &"<redacted>")
            .field("client_id", &"<redacted>")
            .field("user_agent", &self.user_agent)
            .field("accept_language", &self.accept_language)
            .finish()
    }
}

/// OTA 返回所处的绑定阶段。未绑定时严格禁止建立音频连接。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ActivationState {
    Ready,
    Pending,
}

pub(crate) struct OtaConfig {
    mqtt: Option<MqttPayload>,
    activation_state: ActivationState,
    activation_code: Option<String>,
}

struct MqttPayload {
    endpoint: Option<String>,
    client_id: Option<String>,
    username: Option<String>,
    password: Option<String>,
    publish_topic: Option<String>,
    subscribe_topic: Option<String>,
    keepalive: Option<u64>,
}

/// 已验证、但仅限 Rust 后端使用的 MQTT 供应信息。
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct MqttProvisioning {
    endpoint: String,
    client_id: String,
    username: String,
    password: String,
    publish_topic: String,
    subscribe_topic: Option<String>,
    keepalive: u16,
}

impl MqttProvisioning {
    /// Only encrypted local settings may use this recovery path. Re-validate
    /// every stored field so an edited/corrupted local file can never turn
    /// into a network connection attempt.
    pub(crate) fn from_encrypted_json(raw: &str) -> Result<Self, XiaozhiProtocolError> {
        let provisioning: Self = serde_json::from_str(raw)
            .map_err(|_| XiaozhiProtocolError::new("小智 MQTT 配置损坏"))?;
        provisioning.validate()?;
        Ok(provisioning)
    }

    fn validate(&self) -> Result<(), XiaozhiProtocolError> {
        if !valid_mqtt_endpoint(&self.endpoint)
            || self.client_id.trim().is_empty()
            || self.username.trim().is_empty()
            || self.password.trim().is_empty()
            || self.client_id.chars().any(char::is_control)
            || self.username.chars().any(char::is_control)
            || self.password.chars().any(char::is_control)
            || self.publish_topic.trim().is_empty()
            || self.publish_topic.len() > 512
            || self.publish_topic.chars().any(char::is_control)
            || self.subscribe_topic.as_deref().is_some_and(|topic|
                topic.trim().is_empty() || topic.len() > 512 || topic.chars().any(char::is_control))
            || !(1..=3600).contains(&self.keepalive)
        {
            return Err(XiaozhiProtocolError::new("小智 MQTT 配置无效"));
        }
        Ok(())
    }
    pub(crate) fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub(crate) fn client_id(&self) -> &str {
        &self.client_id
    }

    pub(crate) fn username(&self) -> &str {
        &self.username
    }

    pub(crate) fn password(&self) -> &str {
        &self.password
    }

    pub(crate) fn publish_topic(&self) -> &str {
        &self.publish_topic
    }

    pub(crate) fn subscribe_topic(&self) -> Option<&str> {
        self.subscribe_topic.as_deref()
    }

    pub(crate) fn keepalive(&self) -> u16 {
        self.keepalive
    }
}

impl fmt::Debug for MqttProvisioning {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MqttProvisioning")
            .field("endpoint", &"<redacted>")
            .field("client_id", &"<redacted>")
            .field("username", &"<redacted>")
            .field("password", &"<redacted>")
            .field("publish_topic", &"<redacted>")
            .field("subscribe_topic", &self.subscribe_topic.as_ref().map(|_| "<redacted>"))
            .field("keepalive", &self.keepalive)
            .finish()
    }
}

impl OtaConfig {
    pub(crate) fn parse(value: Value) -> Result<Self, XiaozhiProtocolError> {
        let object = value
            .as_object()
            .ok_or_else(|| XiaozhiProtocolError::new("小智 OTA 返回格式无效"))?;

        let mqtt = object
            .get("mqtt")
            .filter(|mqtt| !mqtt.is_null())
            .and_then(Value::as_object)
            .map(|mqtt| MqttPayload {
                endpoint: mqtt
                    .get("endpoint")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                client_id: mqtt
                    .get("client_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                username: mqtt
                    .get("username")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                password: mqtt
                    .get("password")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                publish_topic: mqtt
                    .get("publish_topic")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                subscribe_topic: mqtt
                    .get("subscribe_topic")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                keepalive: mqtt.get("keepalive").and_then(Value::as_u64),
            });

        let activation_code = object
            .get("activation")
            .and_then(Value::as_object)
            .and_then(|activation| activation.get("code"))
            .and_then(Value::as_str)
            .filter(|code| !code.trim().is_empty())
            .map(str::to_owned);

        Ok(Self {
            mqtt,
            activation_state: if object.contains_key("activation") {
                ActivationState::Pending
            } else {
                ActivationState::Ready
            },
            activation_code,
        })
    }

    pub(crate) fn activation_state(&self) -> ActivationState {
        self.activation_state
    }

    pub(crate) fn activation_code(&self) -> Option<&str> {
        self.activation_code.as_deref()
    }

    pub(crate) fn mqtt_provisioning(&self) -> Result<MqttProvisioning, XiaozhiProtocolError> {
        if self.activation_state == ActivationState::Pending {
            return Err(XiaozhiProtocolError::new(
                "小智设备尚未绑定，不能建立语音会话",
            ));
        }
        let mqtt = self
            .mqtt
            .as_ref()
            .ok_or_else(|| XiaozhiProtocolError::new("小智 OTA 未提供 MQTT 配置"))?;
        let endpoint = required_mqtt_field(&mqtt.endpoint)?;
        if !valid_mqtt_endpoint(endpoint) {
            return Err(XiaozhiProtocolError::new("小智 MQTT 地址无效"));
        }
        let client_id = required_mqtt_field(&mqtt.client_id)?;
        let username = required_mqtt_field(&mqtt.username)?;
        let password = required_mqtt_field(&mqtt.password)?;
        let publish_topic = required_mqtt_field(&mqtt.publish_topic)?;
        if publish_topic.len() > 512 || publish_topic.chars().any(char::is_control) {
            return Err(XiaozhiProtocolError::new("小智 MQTT 发布主题无效"));
        }
        let keepalive = mqtt.keepalive.unwrap_or(240);
        let keepalive = u16::try_from(keepalive)
            .ok()
            .filter(|keepalive| (1..=3600).contains(keepalive))
            .ok_or_else(|| XiaozhiProtocolError::new("小智 MQTT keepalive 无效"))?;
        let provisioning = MqttProvisioning {
            endpoint: endpoint.to_owned(),
            client_id: client_id.to_owned(),
            username: username.to_owned(),
            password: password.to_owned(),
            publish_topic: publish_topic.to_owned(),
            subscribe_topic: mqtt.subscribe_topic.clone(),
            keepalive,
        };
        provisioning.validate()?;
        Ok(provisioning)
    }
}

fn required_mqtt_field(value: &Option<String>) -> Result<&str, XiaozhiProtocolError> {
    value
        .as_deref()
        .filter(|value| !value.trim().is_empty() && !value.chars().any(char::is_control))
        .ok_or_else(|| XiaozhiProtocolError::new("小智 MQTT 配置不完整"))
}

fn valid_mqtt_endpoint(endpoint: &str) -> bool {
    if endpoint.len() > 255
        || endpoint.chars().any(char::is_whitespace)
        || endpoint.contains(['/', '?', '#', '@'])
        || endpoint.contains("://")
    {
        return false;
    }
    match endpoint.rsplit_once(':') {
        Some((host, port)) => {
            !host.is_empty()
                && !host.contains(':')
                && port.parse::<u16>().is_ok_and(|port| port != 0)
        }
        None => !endpoint.is_empty() && !endpoint.contains(':'),
    }
}

impl fmt::Debug for OtaConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OtaConfig")
            .field("mqtt", &self.mqtt.as_ref().map(|_| "<redacted>"))
            .field("activation_state", &self.activation_state)
            .field("activation_code", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct XiaozhiProtocolError(&'static str);

impl XiaozhiProtocolError {
    const fn new(message: &'static str) -> Self {
        Self(message)
    }
}

impl fmt::Display for XiaozhiProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl Error for XiaozhiProtocolError {}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use uuid::Uuid;

    #[test]
    fn ota_request_uses_a_local_identity_and_disables_unimplemented_capabilities() {
        let device_id = DeviceId::from_octets([0x02, 0x10, 0x2a, 0x30, 0x4b, 0x5c])
            .expect("locally administered unicast MAC is valid");
        let client_id = Uuid::parse_str("11111111-2222-4333-8444-555555555555").unwrap();

        let ota_request = OtaRequest::new(device_id, client_id, false);
        let headers = ota_request.http_headers();
        assert_eq!(headers.activation_version, "1");
        assert_eq!(headers.device_id, "02:10:2a:30:4b:5c");
        assert_eq!(headers.client_id, "11111111-2222-4333-8444-555555555555");
        assert_eq!(headers.accept_language, "zh-CN");

        let request = serde_json::to_value(ota_request).unwrap();
        assert_eq!(request["version"], 2);
        assert_eq!(request["language"], "zh-CN");
        assert_eq!(request["mac_address"], "02:10:2a:30:4b:5c");
        assert_eq!(request["uuid"], "11111111-2222-4333-8444-555555555555");
        assert_eq!(request["protocol"]["features"]["mcp"], false);
        assert_eq!(request["protocol"]["features"]["aec"], false);

        let enabled = serde_json::to_value(OtaRequest::new(device_id, client_id, true)).unwrap();
        assert_eq!(enabled["protocol"]["features"]["mcp"], true);
    }

    #[test]
    fn activation_response_is_pending_and_cannot_open_audio() {
        let config = OtaConfig::parse(json!({
            "activation": { "code": "123456", "message": "xiaozhi.me\\n123456", "challenge": "opaque" }
        }))
        .expect("activation response should parse");

        assert_eq!(config.activation_state(), ActivationState::Pending);
        assert_eq!(config.activation_code(), Some("123456"));
        assert!(!format!("{config:?}").contains("123456"));
        assert!(
            config.mqtt_provisioning().is_err(),
            "unbound devices must not open audio"
        );
    }

    #[test]
    fn mixed_mqtt_and_websocket_response_uses_only_mqtt_provisioning() {
        let config = OtaConfig::parse(json!({
            "mqtt": {
                "endpoint": "mqtt.example.com:8883",
                "client_id": "test-client",
                "username": "test-user",
                "password": "mqtt-secret",
                "publish_topic": "device/test/up",
                "keepalive": 240
            },
            "websocket": { "url": "wss://voice.example.com/xiaozhi/v1/", "token": "secret-token" }
        }))
        .expect("official mixed response should select MQTT");
        let provisioning = config
            .mqtt_provisioning()
            .expect("MQTT must be usable after binding");

        assert_eq!(provisioning.endpoint(), "mqtt.example.com:8883");
        assert_eq!(provisioning.client_id(), "test-client");
        assert_eq!(provisioning.keepalive(), 240);
        assert!(!format!("{config:?}{provisioning:?}").contains("mqtt-secret"));
        assert!(!format!("{config:?}{provisioning:?}").contains("secret-token"));
    }

    #[test]
    fn incomplete_mqtt_provisioning_cannot_open_audio_without_leaking_credentials() {
        for mqtt in [
            json!({ "client_id": "test-client", "username": "test-user", "password": "mqtt-secret", "publish_topic": "device/test/up" }),
            json!({ "endpoint": "mqtt.example.com:8883", "username": "test-user", "password": "mqtt-secret", "publish_topic": "device/test/up" }),
            json!({ "endpoint": "mqtt.example.com:8883", "client_id": "test-client", "password": "mqtt-secret", "publish_topic": "device/test/up" }),
            json!({ "endpoint": "mqtt.example.com:8883", "client_id": "test-client", "username": "test-user", "publish_topic": "device/test/up" }),
            json!({ "endpoint": "mqtt.example.com:8883", "client_id": "test-client", "username": "test-user", "password": "mqtt-secret" }),
        ] {
            let config = OtaConfig::parse(json!({ "mqtt": mqtt }))
                .expect("OTA response shape itself should remain inspectable");
            let error = config
                .mqtt_provisioning()
                .expect_err("incomplete MQTT config must not open audio");
            assert!(!error.to_string().contains("mqtt-secret"));
        }
    }

    #[test]
    fn invalid_mqtt_endpoint_or_keepalive_is_rejected_without_leaking_credentials() {
        for mqtt in [
            json!({ "endpoint": "mqtts://mqtt.example.com", "client_id": "test-client", "username": "test-user", "password": "mqtt-secret", "publish_topic": "device/test/up" }),
            json!({ "endpoint": "mqtt.example.com:0", "client_id": "test-client", "username": "test-user", "password": "mqtt-secret", "publish_topic": "device/test/up" }),
            json!({ "endpoint": "mqtt.example.com:8883", "client_id": "test-client", "username": "test-user", "password": "mqtt-secret", "publish_topic": "device/test/up", "keepalive": 0 }),
            json!({ "endpoint": "mqtt.example.com:8883", "client_id": "test-client", "username": "test-user", "password": "mqtt-secret", "publish_topic": "device/test/up", "keepalive": 3601 }),
        ] {
            let config = OtaConfig::parse(json!({ "mqtt": mqtt })).unwrap();
            let error = config.mqtt_provisioning().unwrap_err();
            assert!(!error.to_string().contains("mqtt-secret"));
        }
    }

    #[test]
    fn verified_mqtt_provisioning_can_round_trip_only_inside_the_backend_store() {
        let original = OtaConfig::parse(json!({
            "mqtt": {
                "endpoint": "mqtt.example.com:8883",
                "client_id": "test-client",
                "username": "test-user",
                "password": "mqtt-secret",
                "publish_topic": "device/test/up",
                "keepalive": 240
            }
        }))
        .unwrap()
        .mqtt_provisioning()
        .unwrap();

        let restored = MqttProvisioning::from_encrypted_json(
            &serde_json::to_string(&original).unwrap(),
        )
        .expect("encrypted settings may restore a verified MQTT provisioning");
        assert_eq!(restored.endpoint(), original.endpoint());
        assert_eq!(restored.keepalive(), 240);
        assert!(!format!("{restored:?}").contains("mqtt-secret"));
    }

    #[test]
    #[ignore = "仅在用户明确提供小智 OTA 地址并设置 XIAOZHI_LIVE_TEST=1 时运行"]
    fn live_ota_smoke() {
        assert_eq!(
            std::env::var("XIAOZHI_LIVE_TEST").as_deref(),
            Ok("1"),
            "仅允许显式 opt-in 的小智 OTA 测试"
        );
        let ota_url = std::env::var("XIAOZHI_OTA_URL")
            .expect("必须通过 XIAOZHI_OTA_URL 提供用户确认的小智 OTA 地址");
        let device_id = DeviceId::from_octets([0x02, 0x58, 0x49, 0x41, 0x4F, 0x5A])
            .expect("固定测试虚拟 MAC 必须有效");
        let request = OtaRequest::new(
            device_id,
            Uuid::parse_str("06bc6bc1-7c0d-4dc0-a6c7-51b609878adb").unwrap(),
            false,
        );
        let headers = request.http_headers();
        let started = std::time::Instant::now();
        let response = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .expect("OTA 测试 HTTP 客户端必须建立")
            .post(ota_url)
            .header("Activation-Version", headers.activation_version)
            .header("Device-Id", headers.device_id)
            .header("Client-Id", headers.client_id)
            .header("User-Agent", headers.user_agent)
            .header("Accept-Language", headers.accept_language)
            .json(&request)
            .send()
            .expect("小智 OTA 请求失败")
            .error_for_status()
            .expect("小智 OTA 返回 HTTP 错误");
        let config = OtaConfig::parse(response.json::<Value>().expect("小智 OTA 未返回合法 JSON"))
            .expect("小智 OTA 返回格式不兼容");
        println!(
            "xiaozhi OTA accepted: {:?} in {}ms",
            config.activation_state(),
            started.elapsed().as_millis()
        );
        if let Some(code) = config.activation_code() {
            println!("xiaozhi activation code: {code}");
        }
        if config.activation_state() == ActivationState::Ready {
            config
                .mqtt_provisioning()
                .expect("已绑定的小智 OTA 必须返回完整 MQTT 供应");
        }
        assert!(matches!(
            config.activation_state(),
            ActivationState::Pending | ActivationState::Ready
        ));
    }
}
