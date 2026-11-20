/*
 * [Input] 小智 MQTT 控制主题中的 `type:mcp` JSON-RPC 请求。
 * [Output] 有界、双版本兼容的 MCP 请求及回复；此处不执行桌面任务或记录载荷。
 * [Pos] 小智原生设备 MCP 的纯协议边界，供 MQTT transport 和桌面派发桥接共用。
 */

use serde_json::{json, Value};

const MODERN_VERSION: &str = "2026-07-28";
const LEGACY_VERSION: &str = "2024-11-05";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Version { Modern, Legacy }

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ToolName { Dispatch, Status, Result }

impl ToolName {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Dispatch => "desktop_agent.dispatch",
            Self::Status => "desktop_agent.status",
            Self::Result => "desktop_agent.result",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ToolCall {
    pub(crate) tool: ToolName,
    pub(crate) task_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Method {
    Discover,
    Initialize,
    ToolsList,
    ToolCall(ToolCall),
}

#[derive(Clone, Debug)]
pub(crate) struct Request {
    pub(crate) session_id: Option<String>,
    pub(crate) id: Value,
    pub(crate) version: Version,
    pub(crate) method: Method,
}

pub(crate) fn parse_request(value: &Value, active_session: Option<&str>) -> Result<Request, &'static str> {
    if value.get("type").and_then(Value::as_str) != Some("mcp") { return Err("invalid_envelope"); }
    let session_id = optional_session_id(value.get("session_id"))?;
    let payload = value.get("payload").and_then(Value::as_object).ok_or("invalid_payload")?;
    if payload.get("jsonrpc").and_then(Value::as_str) != Some("2.0") { return Err("invalid_jsonrpc"); }
    let id = payload.get("id").filter(valid_id).cloned().ok_or("invalid_id")?;
    let method_name = payload.get("method").and_then(Value::as_str).ok_or("invalid_method")?;
    let params = payload.get("params").and_then(Value::as_object).ok_or("invalid_params")?;
    let version = protocol_version(method_name, params)?;
    let method = match method_name {
        "server/discover" if version == Version::Modern => Method::Discover,
        "initialize" if version == Version::Legacy => Method::Initialize,
        "tools/list" => Method::ToolsList,
        "tools/call" => {
            let current = active_session.ok_or("session_inactive")?;
            if session_id.as_deref() != Some(current) { return Err("session_mismatch"); }
            Method::ToolCall(parse_tool_call(params)?)
        }
        _ => return Err("unsupported_method"),
    };
    Ok(Request { session_id, id, version, method })
}

pub(crate) fn discovery_result() -> Value {
    json!({
        "supportedVersions": [MODERN_VERSION, LEGACY_VERSION],
        "capabilities": {"tools": {}},
        "ttlMs": 60_000,
        "cacheScope": "private"
    })
}

pub(crate) fn initialize_result() -> Value {
    json!({
        "protocolVersion": LEGACY_VERSION,
        "capabilities": {"tools": {}},
        "serverInfo": {"name": "HachimiDock", "version": env!("CARGO_PKG_VERSION")}
    })
}

pub(crate) fn tool_catalog() -> Value {
    json!({"tools": [
        {"name":"desktop_agent.dispatch", "description":"将当前这句已确认的用户语音原文派发给当前跟随的 Codex 或 WorkBuddy；不得提供、改写或猜测任务文本。", "inputSchema":{"type":"object","additionalProperties":false}},
        {"name":"desktop_agent.status", "description":"查询当前跟随桌面 Agent 的最近任务状态或指定任务状态。", "inputSchema": task_id_schema()},
        {"name":"desktop_agent.result", "description":"查询当前跟随桌面 Agent 的最近任务结果或指定任务结果。", "inputSchema": task_id_schema()}
    ]})
}

pub(crate) fn success_reply(request: &Request, result: Value) -> Value {
    let mut result = result;
    if request.version == Version::Modern {
        let object = result.as_object_mut().expect("MCP result must be object");
        object.insert("resultType".into(), json!("complete"));
        object.insert("_meta".into(), json!({"io.modelcontextprotocol/serverInfo":{"name":"HachimiDock","version":env!("CARGO_PKG_VERSION")}}));
    }
    envelope(request, json!({"jsonrpc":"2.0", "id":request.id, "result":result}))
}

pub(crate) fn tool_error_reply(request: &Request, message: &'static str) -> Value {
    let mut result = json!({"content":[{"type":"text","text":message}], "isError":true});
    if request.version == Version::Modern {
        result["resultType"] = json!("complete");
        result["_meta"] = json!({"io.modelcontextprotocol/serverInfo":{"name":"HachimiDock","version":env!("CARGO_PKG_VERSION")}});
    }
    envelope(request, json!({"jsonrpc":"2.0", "id":request.id, "result":result}))
}

fn envelope(request: &Request, payload: Value) -> Value {
    let mut output = json!({"type":"mcp", "payload":payload});
    if let Some(session_id) = &request.session_id { output["session_id"] = json!(session_id); }
    output
}

fn task_id_schema() -> Value {
    json!({"type":"object","additionalProperties":false,"properties":{"taskId":{"type":"string","format":"uuid"}}})
}

fn protocol_version(method: &str, params: &serde_json::Map<String, Value>) -> Result<Version, &'static str> {
    if method == "initialize" {
        return if params.get("protocolVersion").and_then(Value::as_str) == Some(LEGACY_VERSION) { Ok(Version::Legacy) } else { Err("invalid_legacy_version") };
    }
    // 2024-11-05 的小智客户端不会携带 `_meta`；新旧协议需要能并存，
    // 否则绑定在旧版云端的设备会在列工具或调用工具时被误拒绝。
    let Some(meta) = params.get("_meta").and_then(Value::as_object) else {
        return match method {
            "tools/list" | "tools/call" => Ok(Version::Legacy),
            _ => Err("missing_modern_meta"),
        };
    };
    if meta.get("io.modelcontextprotocol/protocolVersion").and_then(Value::as_str) != Some(MODERN_VERSION)
        || !meta.get("io.modelcontextprotocol/clientCapabilities").is_some_and(Value::is_object) {
        return Err("invalid_modern_meta");
    }
    Ok(Version::Modern)
}

fn parse_tool_call(params: &serde_json::Map<String, Value>) -> Result<ToolCall, &'static str> {
    let tool = match params.get("name").and_then(Value::as_str) {
        Some("desktop_agent.dispatch") => ToolName::Dispatch,
        Some("desktop_agent.status") => ToolName::Status,
        Some("desktop_agent.result") => ToolName::Result,
        _ => return Err("unknown_tool"),
    };
    let arguments = params.get("arguments").and_then(Value::as_object).ok_or("invalid_arguments")?;
    let task_id = match arguments.get("taskId") {
        None => None,
        Some(Value::String(id)) if uuid::Uuid::parse_str(id).is_ok() => Some(id.to_owned()),
        _ => return Err("invalid_task_id"),
    };
    if tool == ToolName::Dispatch && !arguments.is_empty() { return Err("dispatch_arguments_forbidden"); }
    if tool != ToolName::Dispatch && arguments.len() != usize::from(task_id.is_some()) { return Err("invalid_arguments"); }
    Ok(ToolCall { tool, task_id })
}

fn optional_session_id(value: Option<&Value>) -> Result<Option<String>, &'static str> {
    match value {
        None => Ok(None),
        Some(Value::String(value)) if !value.is_empty() && value.len() <= 128 && !value.chars().any(char::is_control) => Ok(Some(value.to_owned())),
        _ => Err("invalid_session"),
    }
}

fn valid_id(value: &&Value) -> bool {
    match *value {
        Value::String(value) => !value.is_empty() && value.len() <= 128 && !value.chars().any(char::is_control),
        Value::Number(value) => value.as_i64().is_some() || value.as_u64().is_some(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn modern(method: &str, params: serde_json::Value) -> serde_json::Value {
        json!({
            "type": "mcp",
            "payload": {
                "jsonrpc": "2.0", "id": 7, "method": method,
                "params": params
            }
        })
    }

    fn modern_meta() -> serde_json::Value {
        json!({"_meta": {
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientCapabilities": {}
        }})
    }

    #[test]
    fn modern_discovery_and_catalog_advertise_only_desktop_tools() {
        let request = parse_request(&modern("server/discover", modern_meta()), None).unwrap();
        let reply = success_reply(&request, discovery_result());
        assert_eq!(reply["type"], "mcp");
        assert_eq!(reply["payload"]["result"]["resultType"], "complete");

        let mut params = modern_meta();
        params.as_object_mut().unwrap().insert("withUserTools".into(), json!(false));
        let request = parse_request(&modern("tools/list", params), None).unwrap();
        let reply = success_reply(&request, tool_catalog());
        let tools = reply["payload"]["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.iter().map(|tool| tool["name"].as_str().unwrap()).collect::<Vec<_>>(),
            vec!["desktop_agent.dispatch", "desktop_agent.status", "desktop_agent.result"]);
        assert_eq!(tools[0]["inputSchema"], json!({"type":"object","additionalProperties":false}));
        let error = tool_error_reply(&request, "拒绝");
        assert_eq!(error["payload"]["result"]["resultType"], "complete");
        assert!(error["payload"]["result"]["_meta"].is_object());
    }

    #[test]
    fn legacy_initialize_and_list_keep_legacy_result_shape() {
        let initialize = json!({"type":"mcp","payload":{"jsonrpc":"2.0","id":1,
            "method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{}}}});
        let request = parse_request(&initialize, None).unwrap();
        let reply = success_reply(&request, initialize_result());
        assert_eq!(reply["payload"]["result"]["protocolVersion"], "2024-11-05");
        assert!(reply["payload"]["result"].get("resultType").is_none());

        let list = json!({"type":"mcp","payload":{"jsonrpc":"2.0","id":2,
            "method":"tools/list","params":{}}});
        let request = parse_request(&list, None).unwrap();
        assert_eq!(request.version, Version::Legacy);
        assert!(success_reply(&request, tool_catalog())["payload"]["result"].get("resultType").is_none());
    }

    #[test]
    fn calls_require_current_session_and_dispatch_accepts_no_model_text() {
        let call = json!({"type":"mcp","session_id":"session-a","payload":{"jsonrpc":"2.0","id":"call-1",
            "method":"tools/call","params":{"name":"desktop_agent.dispatch","arguments":{},"_meta":{
                "io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}});
        assert!(matches!(parse_request(&call, Some("session-a")).unwrap().method, Method::ToolCall(_)));
        assert!(parse_request(&call, Some("other")).is_err());
        let mut altered = call;
        altered["payload"]["params"]["arguments"] = json!({"task":"delete everything"});
        assert!(parse_request(&altered, Some("session-a")).is_err());
    }
}
