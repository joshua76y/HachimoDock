/*
 * [Input] 当前小智 MQTT session 的 MCP 工具调用及该 session 最终确认的 STT 原文。
 * [Output] 仅限 desktop_agent 三个只读/派发工具的受限 MCP 内容结果。
 * [Pos] 小智模型与桌面 Agent 的授权边界；绝不接受模型提供的任务正文。
 */

use serde_json::{json, Value};
use std::sync::Mutex;

use crate::desktop_agent::{self, Context, Conversation};
use crate::xiaozhi_mcp::{ToolCall, ToolName};

const MAX_FINAL_TEXT_CHARS: usize = 4096;

struct PendingTurn {
    turn: u32,
    text: String,
}

struct State {
    conversation: Conversation,
    pending: Option<PendingTurn>,
    last_turn: u32,
    consumed_turn: u32,
}

impl Default for State {
    fn default() -> Self {
        Self {
            conversation: Conversation::default(),
            pending: None,
            last_turn: 0,
            consumed_turn: 0,
        }
    }
}

/// A bridge is created only by a live Xiaozhi realtime session. It owns no
/// global speech data and is dropped together with that session.
pub(crate) struct XiaozhiDesktopMcp {
    session_id: String,
    board: String,
    state: Mutex<State>,
}

impl XiaozhiDesktopMcp {
    pub(crate) fn new(session_id: String, board: String) -> Self {
        Self { session_id, board, state: Mutex::new(State::default()) }
    }

    pub(crate) fn session_id(&self) -> &str { &self.session_id }

    /// A final STT event replaces the dispatchable turn. Empty, malformed and
    /// stale events are intentionally ignored; none may revive an old task.
    pub(crate) fn record_final_user_text(&self, turn: u32, text: &str) {
        let text = text.trim();
        if turn == 0 || text.is_empty() || text.chars().count() > MAX_FINAL_TEXT_CHARS
            || text.chars().any(char::is_control) {
            return;
        }
        let Ok(mut state) = self.state.lock() else { return; };
        if turn <= state.last_turn || turn <= state.consumed_turn { return; }
        state.last_turn = turn;
        state.pending = Some(PendingTurn { turn, text: text.to_owned() });
    }

    /// Execute after protocol/session validation. The actual desktop work is
    /// performed outside the lock so a slow accessibility/UI action never
    /// blocks later STT or audio traffic. A dispatch turn is claimed first.
    pub(crate) async fn handle_call(&self, call: ToolCall) -> Value {
        match call.tool {
            ToolName::Dispatch => match self.claim_dispatch() {
                Ok(context) => Self::safe_agent_result(desktop_agent::execute(&context, json!({"operation":"dispatch"}), false).await),
                Err(reason) => tool_error(reason),
            },
            ToolName::Status | ToolName::Result => {
                let operation = if call.tool == ToolName::Status { "status" } else { "result" };
                let mut input = json!({"operation": operation});
                if let Some(task_id) = call.task_id { input["taskId"] = json!(task_id); }
                let context = self.read_context();
                Self::safe_agent_result(desktop_agent::execute(&context, input, false).await)
            }
        }
    }

    fn claim_dispatch(&self) -> Result<Context, &'static str> {
        let mut state = self.state.lock().map_err(|_| "桌面任务状态暂不可用")?;
        let pending = state.pending.take().ok_or("本轮还没有可确认派发的语音内容")?;
        if pending.turn <= state.consumed_turn { return Err("这句语音已经派发过，不会重复发送"); }
        state.consumed_turn = pending.turn;
        // Snapshot the prior conversation before adding this exact user turn;
        // this matches desktop_agent's own real-time voice call convention.
        let context = Context::new(&self.session_id, &self.board, pending.turn, &pending.text)
            .with_conversation(&state.conversation)
            .with_followed_agent();
        state.conversation.push("user", &pending.text);
        Ok(context)
    }

    fn read_context(&self) -> Context {
        let Ok(state) = self.state.lock() else {
            return Context::new(&self.session_id, &self.board, 0, "").with_followed_agent();
        };
        Context::new(&self.session_id, &self.board, state.last_turn, "")
            .with_conversation(&state.conversation)
            .with_followed_agent()
    }

    /// desktop_agent receipts may contain a live draft or Agent reply summary.
    /// The XiaoZhi model only needs a bounded state/identifier, never that text.
    fn safe_agent_result(value: Value) -> Value {
        if value.get("errorCode").is_some() { return tool_error("桌面 Agent 暂时无法处理，请在电脑端检查后再问进度"); }
        let task_id = value.get("taskId").and_then(Value::as_str).unwrap_or("");
        let state = value.get("state").and_then(Value::as_str).unwrap_or("unknown");
        let agent = value.get("agent").and_then(Value::as_str).unwrap_or("desktop");
        let suffix = if task_id.is_empty() { String::new() } else { format!("，编号 {task_id}") };
        tool_text(format!("桌面 Agent {agent} 的任务状态为 {state}{suffix}。请在电脑端查看具体内容。"))
    }
}

fn tool_text(text: String) -> Value {
    json!({"content":[{"type":"text","text":text}],"isError":false})
}

fn tool_error(text: &'static str) -> Value {
    json!({"content":[{"type":"text","text":text}],"isError":true})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_claims_only_the_latest_final_text_once() {
        let bridge = XiaozhiDesktopMcp::new("xiaozhi-session".into(), "board-a".into());
        assert!(bridge.claim_dispatch().is_err());
        bridge.record_final_user_text(1, "  在当前项目里修复测试  ");
        let context = bridge.claim_dispatch().unwrap();
        assert_eq!(context.request, "在当前项目里修复测试");
        assert_eq!(context.conversation, "xiaozhi-session");
        assert!(bridge.claim_dispatch().is_err());
    }

    #[test]
    fn stale_or_invalid_stt_cannot_replace_or_revive_a_turn() {
        let bridge = XiaozhiDesktopMcp::new("session-a".into(), "board-a".into());
        bridge.record_final_user_text(2, "最新原话");
        bridge.record_final_user_text(1, "旧原话");
        bridge.record_final_user_text(3, " ");
        assert_eq!(bridge.claim_dispatch().unwrap().request, "最新原话");
        bridge.record_final_user_text(2, "重复原话");
        assert!(bridge.claim_dispatch().is_err());
    }

    #[test]
    fn model_call_carries_no_text_and_status_only_forwards_uuid() {
        let dispatch = ToolCall { tool: ToolName::Dispatch, task_id: None };
        assert_eq!(dispatch.tool, ToolName::Dispatch);
        let bridge = XiaozhiDesktopMcp::new("session-a".into(), "board-a".into());
        let context = bridge.read_context();
        assert!(context.request.is_empty());
        assert_eq!(bridge.session_id(), "session-a");
    }

    #[test]
    fn result_text_never_echoes_desktop_message_or_request() {
        let reply = XiaozhiDesktopMcp::safe_agent_result(json!({
            "taskId":"3fa85f64-5717-4562-b3fc-2c963f66afa6", "agent":"codex", "state":"done",
            "message":"秘密草稿：删除一切", "resultExcerpt":"私密回答"
        }));
        let text = reply["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("done"));
        assert!(!text.contains("秘密草稿"));
        assert!(!text.contains("私密回答"));
    }
}
