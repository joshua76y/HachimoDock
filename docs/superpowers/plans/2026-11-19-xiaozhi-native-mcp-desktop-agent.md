# 小智原生 MCP 桌面任务派发 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让已绑定的小智实时对话可通过原生 MQTT MCP 工具把当前语音任务安全派发给当前跟随的 Codex 或 WorkBuddy。

**Architecture:** 保持现有 MQTT + UDP 音频会话，HachimiDock 在 MQTT 控制主题上实现官方 MCP Server 的发现、工具目录、调用及 JSON-RPC 回复。独立的桌面任务桥接器只绑定当前 session 的最终 STT 原文，并复用 `desktop_agent` 的派发与回执机制；前端开关决定是否在 OTA 与 hello 中宣告 MCP 能力。

**Tech Stack:** Rust / Tauri 2、Tokio、rumqttc、serde_json、React、Vitest/Node 测试。

**Spec:** `docs/superpowers/specs/2026-11-19-xiaozhi-native-mcp-design.md`

## Global Constraints

- 仅“小智 AI”实时引擎启用此能力；API 与本地 OMLX 路径不得改变。
- MCP 使用现有 MQTT 控制主题；UDP 只承载音频。
- 开关默认关闭，生效必须重新进入小智实时会话。
- 兼容 MCP `2026-07-28` 与 `2024-11-05` 的发现、目录与调用格式。
- 派发必须使用本轮已识别的最终 STT 原文，拒绝模型提供或改写的任务文本。
- 首版只开放 `desktop_agent.dispatch`、`desktop_agent.status`、`desktop_agent.result`；不开放取消。
- MCP 失败、目录读取失败不得中断正常小智语音对话，不自动重试有副作用调用。
- 诊断日志不得保留任务正文、工具参数、转录、PCM、MCP payload 或任何凭据。
- 不在此功能中新增外部小智 MCP 端点、桥接进程或控制台端点凭据。

## Review Focus

- 旧版或现代 MCP 握手：未带 session 的发现/目录请求要得到正确格式回复；测试归入 Task 2。
- 重放/迟到的 `dispatch`：已消费、过期或不属于当前会话的 STT 不得再次写入桌面 Agent；测试归入 Task 3。
- 模型伪造任务文本：工具 schema 不得存在可执行文本参数，派发必须只使用保存的原话；测试归入 Task 3。
- 长时间目标 Agent 预检：MCP 处理不能阻塞音频转发和 MQTT 事件循环；测试归入 Task 4。
- 开关关闭或实时会话切换：不宣告 MCP，旧调用不得跨 session/设备执行；测试归入 Task 1 与 Task 4。

---

## File Structure

- `pc/src-tauri/src/xiaozhi_protocol.rs`：OTA MCP 能力字段的安全序列化。
- `pc/src-tauri/src/xiaozhi_settings.rs`：开关的加密持久化、脱敏状态与运行期能力读取。
- `pc/src/ApiSettings.jsx`、`pc/src/ApiSettings.test.js`：小智设置开关及“重新进入会话后生效”提示。
- `pc/src-tauri/src/xiaozhi_mcp.rs`（新增）：纯 JSON-RPC 请求验证、双版本 discovery/list/call 解析、工具 schema 与无敏感内容的回复构造。
- `pc/src-tauri/src/xiaozhi_desktop_mcp.rs`（新增）：session 绑定的最终 STT 原话、桌面 Agent Context 创建、一次性派发、状态/结果调用。
- `pc/src-tauri/src/xiaozhi_realtime.rs`：在 MQTT 事件循环中收发 `type:"mcp"`，把耗时调用从音频循环中隔离。
- `pc/src-tauri/src/realtime_chat.rs`：将小智最终 STT 与会话生命周期交给桥接器、提交 MCP 回复、显示工具调用结果。
- `pc/src-tauri/src/lib.rs`：注册新模块（不新增 Tauri 命令）。
- `pc/src-tauri/src/realtime_chat_log.rs`：仅在必要时扩展封闭诊断枚举。

### Task 1: 小智 MCP 设置与能力协商

**Files:**
- Modify: `pc/src-tauri/src/xiaozhi_protocol.rs`
- Modify: `pc/src-tauri/src/xiaozhi_settings.rs`
- Modify: `pc/src-tauri/src/xiaozhi_realtime.rs`
- Modify: `pc/src/ApiSettings.jsx`
- Modify: `pc/src/ApiSettings.test.js`

**Interfaces:**
- Produces: `xiaozhi_settings::mcp_enabled() -> Result<bool, String>`，以及 `XiaozhiRealtime::connect(provisioning: MqttProvisioning, mcp_enabled: bool) -> Result<XiaozhiRealtime, String>`。
- Consumes: `OtaRequest` 的 MCP feature 值和 `XiaozhiStatus.mcp_enabled`。

- [ ] **Step 1: 写入设置迁移与开关默认关闭的失败测试**

在 `xiaozhi_settings.rs` 的测试中覆盖：旧加密配置不含字段时加载为 `false`；保存 `mcpEnabled: true` 后 status 只返回布尔值、仍不暴露 MAC/MQTT 密码；切换 OTA URL 保留开关但清除旧供应配置。

- [ ] **Step 2: 运行设置测试，确认因字段/接口尚不存在而失败**

Run: `cargo test xiaozhi_settings::tests --lib --quiet`

Expected: FAIL，缺少 `mcp_enabled` 或相应断言失败。

- [ ] **Step 3: 实现持久化设置、OTA/hello 能力传递与前端开关**

为 `XiaozhiSettings` 添加 serde 默认值 `mcp_enabled: bool`；`XiaozhiSettingsInput` 用可选字段保留未提交值；`XiaozhiStatus` 仅暴露该布尔值。将 `OtaRequest` 改为显式接收该值，在 `protocol.features.mcp` 中序列化；`client_hello(mcp_enabled)` 仅在开关开启时声明 `mcp: true`。在 React 中保存开关，显示“下次进入小智实时对话后生效”，不改变其它引擎设置。

- [ ] **Step 4: 运行 Rust 与前端针对性测试，确认通过**

Run: `cargo test xiaozhi_settings::tests xiaozhi_protocol::tests xiaozhi_realtime::tests --lib --quiet && npm test -- --run ApiSettings.test.js`

Expected: PASS；关闭状态的 OTA/hello 都是 `mcp:false`，开启状态均为 `mcp:true`。

- [ ] **Step 5: 暂存本任务明确文件并提交**

仅在确认这些文件不是用户未提交工作后执行：

```bash
git add pc/src-tauri/src/xiaozhi_protocol.rs pc/src-tauri/src/xiaozhi_settings.rs pc/src-tauri/src/xiaozhi_realtime.rs pc/src/ApiSettings.jsx pc/src/ApiSettings.test.js
git commit -m "feat: add Xiaozhi MCP capability setting"
```

### Task 2: 双版本 MQTT MCP 协议核心

**Files:**
- Create: `pc/src-tauri/src/xiaozhi_mcp.rs`
- Modify: `pc/src-tauri/src/lib.rs`
- Test: `pc/src-tauri/src/xiaozhi_mcp.rs` 内部 `#[cfg(test)]` 模块

**Interfaces:**
- Produces: `parse_request(envelope: &Value, active_session: Option<&str>) -> ParsedRequest`。
- Produces: `tool_catalog() -> Value`、`success_reply(request: &ParsedRequest, result: Value) -> Value`、`error_reply(request: &ParsedRequest, code: i64, message: &'static str) -> Value`。
- Consumes: `ParsedRequest::{Discover, Initialize, ToolsList, ToolCall}`，其中 `ToolCall` 只允许 `desktop_agent.dispatch/status/result`。

- [ ] **Step 1: 为现代与旧版格式写失败测试**

覆盖现代 `server/discover`、现代和旧版 `tools/list`、三项合法 `tools/call`、无 session 的 warmup、带错 session 的调用、未知工具、带未声明字段的 `dispatch` 参数、非法 JSON-RPC ID，以及每项错误的请求 ID 原样回传。

- [ ] **Step 2: 运行协议测试并确认失败**

Run: `cargo test xiaozhi_mcp::tests --lib --quiet`

Expected: FAIL，模块不存在。

- [ ] **Step 3: 实现 `xiaozhi_mcp` 的纯协议边界**

实现上列接口：现代回复带 `resultType:"complete"` 和官方 `2026-07-28` 元数据；旧版保留旧结构。`desktop_agent.dispatch` schema 是空 object 且 `additionalProperties:false`，从根源拒绝模型传入任务文本；`status/result` 只接受可选 UUID `taskId`。所有 parser 上限为现有 MQTT JSON 长度限制，不解析或记录任何凭据。

- [ ] **Step 4: 运行协议测试并确认通过**

Run: `cargo test xiaozhi_mcp::tests --lib --quiet`

Expected: PASS。

- [ ] **Step 5: 暂存本任务明确文件并提交**

```bash
git add pc/src-tauri/src/xiaozhi_mcp.rs pc/src-tauri/src/lib.rs
git commit -m "feat: implement Xiaozhi MQTT MCP protocol"
```

### Task 3: 绑定语音原文的桌面任务 MCP 服务

**Files:**
- Create: `pc/src-tauri/src/xiaozhi_desktop_mcp.rs`
- Modify: `pc/src-tauri/src/desktop_agent.rs`（仅补充最小的可测试只读任务查询接口，如现有可见性不足）
- Test: `pc/src-tauri/src/xiaozhi_desktop_mcp.rs` 内部 `#[cfg(test)]` 模块

**Interfaces:**
- Produces: `XiaozhiDesktopMcp::new(session_id: String, board: String) -> Self`。
- Produces: `record_final_user_text(&self, turn: u32, text: &str)` 与 `handle_call(&self, call: ToolCall) -> impl Future<Output = Value>`。
- Consumes: `desktop_agent::Context::new(...).with_conversation(...).with_followed_agent()` 与 `desktop_agent::execute(&Context, Value, false)`。

- [ ] **Step 1: 写入授权绑定与重复派发失败测试**

测试 `dispatch`：无最终 STT、空文本、同一 turn 第二次、过期 turn、会话 ID 不匹配、当前目标不支持时都返回 MCP 工具错误且不产生可派发 Context；合法调用的 `Context.request` 必须等于保存的最终原话，绝不可来自 `ToolCall.arguments`。测试 `status/result` 对 UUID、缺失 ID 的转发规则。

- [ ] **Step 2: 运行桥接器测试并确认失败**

Run: `cargo test xiaozhi_desktop_mcp::tests --lib --quiet`

Expected: FAIL，模块不存在。

- [ ] **Step 3: 实现 `XiaozhiDesktopMcp`**

用受互斥保护的 session-local 状态保存有限长度 `Conversation`、最后一个最终 STT 和已消费的 turn。`dispatch` 在持锁时原子地声明该 turn，再释放锁后调用 `desktop_agent::execute`；这保证模型重试不会产生二次输入，也不会让等待桌面输入框阻塞状态锁。工具结果只转换为 MCP `content[text]` 的受限状态消息；不回显手头任务正文或桌面草稿。

- [ ] **Step 4: 运行桥接器与既有桌面 Agent 测试**

Run: `cargo test xiaozhi_desktop_mcp::tests desktop_agent::tests --lib --quiet`

Expected: PASS。

- [ ] **Step 5: 暂存本任务明确文件并提交**

```bash
git add pc/src-tauri/src/xiaozhi_desktop_mcp.rs pc/src-tauri/src/desktop_agent.rs
git commit -m "feat: bridge Xiaozhi MCP calls to desktop agents"
```

### Task 4: 实时会话集成与非阻塞回复

**Files:**
- Modify: `pc/src-tauri/src/xiaozhi_realtime.rs`
- Modify: `pc/src-tauri/src/realtime_chat.rs`
- Modify: `pc/src-tauri/src/realtime_chat_log.rs`
- Test: `pc/src-tauri/src/xiaozhi_realtime.rs` 和 `pc/src-tauri/src/realtime_chat.rs` 内部测试模块

**Interfaces:**
- Consumes: `xiaozhi_mcp::parse_request`、`XiaozhiDesktopMcp`、`XiaozhiRealtime::send_mcp_reply(reply: Value)`。
- Produces: `XiaozhiEvent::McpRequest(ParsedRequest)` 与不阻塞命令通道的 MCP reply 路径。

- [ ] **Step 1: 写入事件循环与隐私日志失败测试**

测试：MCP discovery/list 不带 session 可由 event loop 接收并回复；工具调用只接受当前 session；`UserText` 会更新桥接器；一个未完成的桌面派发不会阻止 PCM 入队或 `tts`/`stt` 事件处理；诊断保存 `tool/operation/ok/errorKind/elapsedMs` 等封闭字段但过滤任务文本、JSON-RPC 参数、session 外标识。

- [ ] **Step 2: 运行集成测试并确认失败**

Run: `cargo test xiaozhi_realtime::tests realtime_chat::tests realtime_chat_log::tests --lib --quiet`

Expected: FAIL，缺少 MCP event/reply 或隐私断言。

- [ ] **Step 3: 接入 MQTT 控制消息与会话循环**

在 `xiaozhi_realtime` 将 `type:"mcp"` 解析为 `XiaozhiEvent::McpRequest`，新增可克隆发送端 `McpReplyHandle`（或等价内部命令）供独立 worker 发布回复。`run_xiaozhi_session` 创建 `XiaozhiDesktopMcp`；每个最终 `UserText` 录入 bridge；收到工具请求后立即在独立 Tokio 任务中调用 bridge，并经 handle 发布 JSON-RPC 回复。音频、MQTT poll 与 session 结束仍由主循环持续处理；结束时取消/忽略旧 worker 的回复。新增最小、安全的 `xiaozhi_mcp` 诊断事件，不写入原始内容。

- [ ] **Step 4: 运行集成、回归与格式检查**

Run: `cargo test xiaozhi_mcp::tests xiaozhi_desktop_mcp::tests xiaozhi_realtime::tests realtime_chat::tests realtime_chat_log::tests --lib --quiet && npm test -- --run ApiSettings.test.js && git diff --check`

Expected: PASS；`git diff --check` 无空白错误。已知工作区既有 warning 不作为本任务通过条件。

- [ ] **Step 5: 构建并进行硬件验收**

Run: `node scripts/build-development.mjs macos`

Expected: 生成已签名的 macOS app。替换唯一固定安装包 `/Applications/HachimiDock.app` 后：开启开关并重新进入小智会话，先执行明确桌面任务、再问进度和结果；关闭开关后重新进入会话，确认小智普通问答正常且工具不可用。

- [ ] **Step 6: 暂存本任务明确文件并提交**

```bash
git add pc/src-tauri/src/xiaozhi_realtime.rs pc/src-tauri/src/realtime_chat.rs pc/src-tauri/src/realtime_chat_log.rs
git commit -m "feat: enable Xiaozhi MCP desktop dispatch"
```

## Self-Review

- 设计中的设置隔离、双版本协议、三项工具、会话/STT 授权、无取消、异步不阻塞、隐私日志、失败降级与物理验收均分别由 Task 1–4 覆盖。
- 关键边界的接口在产出任务中先定义，随后任务仅消费这些接口；工具 schema 和桥接器均禁止模型任务文本。
- 计划未要求格式化整个已存在且大量非格式化的工作区；只验证触及文件及构建产物。
- 每个任务包含先失败的测试、最小实现、通过验证与只暂存明确文件的提交步骤；若工作区归属检查失败，停止提交而不混入用户改动。
