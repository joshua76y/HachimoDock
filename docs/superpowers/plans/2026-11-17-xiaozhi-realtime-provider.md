# 小智 AI 实时问答提供方 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在不替换 ESP32-P4 固件、不影响 API/OMLX 的前提下，加入可配置、可激活的小智 AI 实时语音问答模式。

**Architecture:** 小智是独立的实时音频后端，不是 OpenAI LLM 预设。PC 保持现有 P4 的 16 kHz PCM、字幕与播放控制，新增协议层完成小智 OTA/激活、Opus WebSocket 会话和状态映射；`realtime_chat` 只在已选择小智时转交该后端。API/OMLX 仍走现有 ASR → LLM → TTS 管线且不引用小智模块。

**Tech Stack:** Rust/Tauri 2、现有 `reqwest`/`tokio-tungstenite`、Opus 编解码依赖、React、现有加密配置与 P4 PCM 传输。

**Spec:** `docs/superpowers/specs/2026-11-17-xiaozhi-realtime-provider-design.md`

## Global Constraints

- 不刷写或修改当前 ESP32-P4 固件；USB 优先与 Wi-Fi 回退传输保持不变。
- API 和 OMLX 的现有实时对话策略、配置和工具路径不得被小智引用或修改。
- 小智第一版不得接入 Agent、桌面派发、MCP、本地工具、智能家居、待办、音乐或搜索。
- 音频、转写文本、访问令牌、虚拟设备身份不得写入日志、Tauri 状态快照或导出诊断。
- 仅允许小智配置的 OTA 地址发出身份/激活请求；协议确认后才向其 WebSocket 发送音频。
- 小智服务的免费额度、可用性和账号政策是外部条件，客户端必须给出可操作失败状态而非声称已连接。

## Review Focus

- OTA 返回 `activation` 但没有 WebSocket token：显示待绑定，不尝试建立音频连接；由任务 1 的解析与状态测试覆盖。
- OTA 返回 MQTT 与 WebSocket 同时存在：拒绝而非误选 MQTT；由任务 1 的协议配置测试覆盖。
- 服务端协商的 Opus 采样率不是 16 kHz：先解码再重采样为 P4 的 16 kHz；由任务 4 的 24→16 kHz 帧测试覆盖。
- P4 断线、用户停止、切换引擎期间收到迟到 Opus/TTS：不再发送播放帧且关闭会话；由任务 5 的取消测试覆盖。
- 小智选择状态下 API/OMLX 的 Key、模型名和工具上下文：不出现在任何小智请求或日志中；由任务 2、5 的请求构造与隔离测试覆盖。

---

## File Structure

- `pc/src-tauri/src/xiaozhi_protocol.rs` — OTA/激活/hello JSON 的无网络类型、解析与安全校验。
- `pc/src-tauri/src/xiaozhi_settings.rs` — 加密持久化、虚拟身份、状态、OTA 激活/握手探测的 Tauri 命令。
- `pc/src-tauri/src/xiaozhi_realtime.rs` — Opus 编解码、WebSocket 会话、事件流与取消语义。
- `pc/src-tauri/src/voice_chat_settings.rs` — 只保存实时引擎选择；保留既有 LLM 配置语义。
- `pc/src-tauri/src/realtime_chat.rs` — 根据引擎选择现有管线或小智会话，并复用 P4 HUD/播放器。
- `pc/src-tauri/src/lib.rs` — 模块配置与 Tauri 命令注册。
- `pc/src/lib/api-configuration.js`、`pc/src/ApiSettings.jsx` — 引擎选择、小智配置/激活/测试状态。
- `pc/src/ApiSettings.test.js`、相关 Rust 单元测试 — 配置隔离、协议、音频、取消回归。
- `pc/src-tauri/Cargo.toml`、`pc/src-tauri/Cargo.lock`、`licenses/DESKTOP-RUNTIME-DEPENDENCIES.json`、`pc/scripts/dependency-license-resources.mjs` — 审查并固定 Opus 依赖及许可证清单。
- `pc/docs/realtime-chat.md`、`pc/.folder.md`、`firmware/protocol.md` — 记录 PC 侧小智路径和“不改动固件”的协议边界。

### Task 1: 小智 OTA/激活兼容性门槛

**Files:**
- Create: `pc/src-tauri/src/xiaozhi_protocol.rs`
- Modify: `pc/src-tauri/src/lib.rs`
- Test: `pc/src-tauri/src/xiaozhi_protocol.rs` 的 `#[cfg(test)]` 模块

**Interfaces:**
- Produces: `OtaRequest::new(device_id: DeviceId, client_id: Uuid) -> OtaRequest`、`OtaConfig::parse(Value) -> Result<OtaConfig, XiaozhiProtocolError>`、`ActivationState`。
- Consumes: 仅 `reqwest` JSON 类型；不得访问当前 LLM、ASR、TTS 或 P4 状态。

- [ ] **Step 1: 为 OTA 配置响应创建失败测试**

覆盖：含 `activation` 的响应为 `Pending`；同时存在 MQTT/WebSocket、无 token、非 `wss://` 地址、带用户信息的 URL 都被拒绝；错误文本不包含 token。

- [ ] **Step 2: 运行协议测试确认失败**

Run: `cargo test xiaozhi_protocol::tests --lib --quiet`

Expected: FAIL，因为 `xiaozhi_protocol` 模块尚不存在。

- [ ] **Step 3: 实现纯解析协议层**

实现上列接口；OTA 请求始终使用版本 2、`zh-CN`、不宣告 MCP/AEC，且虚拟身份使用本地管理 MAC 格式。只接受单独的 WebSocket 配置和 `wss` URL；将服务 token 保持在 `OtaConfig` 私有字段，不实现日志输出。

- [ ] **Step 4: 运行协议测试确认通过**

Run: `cargo test xiaozhi_protocol::tests --lib --quiet`

Expected: PASS。

- [ ] **Step 5: 添加显式 opt-in 的无音频验收测试**

测试仅在 `XIAOZHI_LIVE_TEST=1`、`XIAOZHI_OTA_URL` 明确提供时运行；向该地址发送最小 OTA 请求，记录仅状态分类与耗时。收到待激活状态时通过并输出绑定码到调用者，不发送 WebSocket 音频。

- [ ] **Step 6: 执行兼容性门槛**

Run: `XIAOZHI_LIVE_TEST=1 XIAOZHI_OTA_URL='<用户提供的小智 OTA 地址>' cargo test xiaozhi_protocol::tests::live_ota_smoke --lib -- --ignored --nocapture`

Expected: 得到 `Pending` 或 `Ready`；若服务拒绝 PC 虚拟身份，停止后续实现并记录服务端状态，不猜测绕过方法。

- [ ] **Step 7: Commit**

```bash
git add pc/src-tauri/src/xiaozhi_protocol.rs pc/src-tauri/src/lib.rs
git commit -m "feat: validate xiaozhi OTA protocol"
```

### Task 2: 小智独立设置与实时引擎选择

**Files:**
- Create: `pc/src-tauri/src/xiaozhi_settings.rs`
- Modify: `pc/src-tauri/src/voice_chat_settings.rs`
- Modify: `pc/src-tauri/src/lib.rs`
- Test: `pc/src-tauri/src/xiaozhi_settings.rs`、`pc/src-tauri/src/voice_chat_settings.rs`

**Interfaces:**
- Consumes: `xiaozhi_protocol::{OtaRequest, OtaConfig, ActivationState}`。
- Produces: `XiaozhiStatus`（不含 token/身份）、`save_xiaozhi_settings`、`activate_xiaozhi`、`test_xiaozhi_connection`；`RealtimeEngine::{LlmPipeline, Xiaozhi}` 与 `voice_chat_settings::realtime_engine() -> Result<RealtimeEngine, String>`。

- [ ] **Step 1: 为设置隔离与迁移写失败测试**

覆盖：旧配置缺少 `realtimeEngine` 时默认为 `LlmPipeline`；切换到小智不改写 `llmProvider`、模型、Key 或 TTS/ASR 设置；`XiaozhiStatus` 序列化不包含 OTA token、完整虚拟 MAC 或 Client ID。

- [ ] **Step 2: 运行设置测试确认失败**

Run: `cargo test 'xiaozhi_settings::tests|voice_chat_settings::tests' --lib --quiet`

Expected: FAIL，因为引擎枚举和小智设置接口尚不存在。

- [ ] **Step 3: 实现独立加密设置与状态命令**

在独立的 `xiaozhi-settings.json` 使用现有 `secure_config` 机制保存 OTA 地址、服务返回的 WebSocket token、持久 Client ID 和虚拟 Device ID。只向前端返回掩码、绑定码、连接阶段和人可读错误。`VoiceChatSettings` 仅新增无凭据的 `realtime_engine`，不得把小智当作 OpenAI 预设。

- [ ] **Step 4: 运行设置测试确认通过**

Run: `cargo test 'xiaozhi_settings::tests|voice_chat_settings::tests' --lib --quiet`

Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git add pc/src-tauri/src/xiaozhi_settings.rs pc/src-tauri/src/voice_chat_settings.rs pc/src-tauri/src/lib.rs
git commit -m "feat: add isolated xiaozhi settings"
```

### Task 3: 设置页的小智配置与激活流程

**Files:**
- Modify: `pc/src/ApiSettings.jsx`
- Modify: `pc/src/lib/api-configuration.js`
- Modify: `pc/src/ApiSettings.test.js`

**Interfaces:**
- Consumes: Tauri 命令 `load_xiaozhi_settings`、`save_xiaozhi_settings`、`activate_xiaozhi`、`test_xiaozhi_connection` 与 `RealtimeEngine` 状态。
- Produces: API/OMLX/小智三种实时引擎的选择 UI；小智的地址保存、绑定码显示和无音频连接测试。

- [ ] **Step 1: 写前端状态与隔离失败测试**

断言：小智是独立实时引擎而非 `LLM_PRESETS` 的 OpenAI 端点；选择小智时不显示或提交 API Key/模型目录控件；切回现有模型链路时保留之前的 API/OMLX 草稿与已保存状态。

- [ ] **Step 2: 运行前端测试确认失败**

Run: `node --test src/ApiSettings.test.js`

Expected: FAIL，因为小智引擎状态和命令尚未接入页面。

- [ ] **Step 3: 实现“小智 AI”设置卡**

在“对话大模型”之前展示“实时对话引擎”：现有模型链路（其下仍可选择云 API 或本地 OMLX）和小智 AI。小智卡只接受 OTA 配置地址，提供“保存并获取绑定状态”“完成绑定后重试”“测试连接”操作；不将服务 token 或身份渲染到 DOM。

- [ ] **Step 4: 运行前端测试确认通过**

Run: `node --test src/ApiSettings.test.js`

Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git add pc/src/ApiSettings.jsx pc/src/lib/api-configuration.js pc/src/ApiSettings.test.js
git commit -m "feat: configure xiaozhi realtime engine"
```

### Task 4: Opus 编解码与小智 WebSocket 会话

**Files:**
- Create: `pc/src-tauri/src/xiaozhi_realtime.rs`
- Modify: `pc/src-tauri/Cargo.toml`
- Modify: `pc/src-tauri/Cargo.lock`
- Modify: `licenses/DESKTOP-RUNTIME-DEPENDENCIES.json`
- Modify: `pc/scripts/dependency-license-resources.mjs`
- Test: `pc/src-tauri/src/xiaozhi_realtime.rs`

**Interfaces:**
- Consumes: `xiaozhi_settings::resolved_session() -> Result<XiaozhiSessionConfig, String>` 和现有 `llm_network::connect_websocket`。
- Produces: `XiaozhiRealtime::connect(config) -> Result<Self, String>`、`start_listening()`、`push_pcm_16k(&[u8])`、`next_event()`、`abort()`、`close()`；事件为 `Transcript(String)`、`Subtitle(String)`、`AudioPcm(Vec<u8>)`、`Speaking(bool)`、`Closed(String)`。

- [ ] **Step 1: 选择并锁定可随 macOS/Windows 打包的 Opus 实现**

优先评估 `opus` 0.4 与其构建型 `opusic-sys` 依赖；确认 release 构建能静态产出并可记录许可证。若无法同时支持 macOS/Windows，停止并选择纯 Rust 实现，不能依赖用户机器已安装的 `libopus`。

- [ ] **Step 2: 写失败测试**

使用固定的 20 ms 16 kHz PCM 测试：三个输入帧聚合为一个 60 ms Opus 帧、解码结果长度正确；24 kHz 下行 PCM 重采样为 16 kHz；含未知 JSON 类型、错误 session ID、非法二进制帧都不产生 `AudioPcm`。

- [ ] **Step 3: 运行音频/协议测试确认失败**

Run: `cargo test xiaozhi_realtime::tests --lib --quiet`

Expected: FAIL，因为 Opus 会话模块尚不存在。

- [ ] **Step 4: 实现有界 WebSocket 会话**

发送 protocol v1 的 hello 与 `listen start/stop`；输入侧把 P4 的 20 ms PCM 聚合成 60 ms 16 kHz 单声道 Opus。仅接受当前小智 session 的 `stt`、`tts` 和二进制 Opus；服务端 24 kHz 下行须先解码并重采样为 16 kHz。`abort`、Socket 关闭和状态错误必须终止所有发送/播放。忽略并记录类别化 MCP、system、custom 消息，绝不执行它们。

- [ ] **Step 5: 更新许可证清单并验证打包前置条件**

Run: `node -e "import('./scripts/dependency-license-resources.mjs').then(m => console.log(JSON.stringify(m.verifyDesktopLicenseInventory('..'))))" && node scripts/desktop-license-inventory.test.mjs`

Expected: 许可证覆盖完整，全部测试通过。

- [ ] **Step 6: 运行音频/协议测试确认通过**

Run: `cargo test xiaozhi_realtime::tests --lib --quiet`

Expected: PASS。

- [ ] **Step 7: Commit**

```bash
git add pc/src-tauri/src/xiaozhi_realtime.rs pc/src-tauri/Cargo.toml pc/src-tauri/Cargo.lock licenses/DESKTOP-RUNTIME-DEPENDENCIES.json pc/scripts/dependency-license-resources.mjs
git commit -m "feat: add xiaozhi opus realtime transport"
```

### Task 5: 将小智会话接入实时对话编排器

**Files:**
- Modify: `pc/src-tauri/src/realtime_chat.rs`
- Modify: `pc/src-tauri/src/voice_chat_settings.rs`
- Test: `pc/src-tauri/src/realtime_chat.rs`

**Interfaces:**
- Consumes: `voice_chat_settings::realtime_engine()` 与 `xiaozhi_realtime::XiaozhiRealtime`。
- Produces: `run_xiaozhi_session(...) -> Result<(), String>`；复用现有 `Session`、`PlayerCmd`、`user_caption`、`hud`、`publish` 和取消机制。

- [ ] **Step 1: 写编排隔离与取消失败测试**

覆盖：选择 `LlmPipeline` 时仍创建现有 ASR/LLM/TTS 链路，选择小智时不调用 `StreamingSpeechRecognizer`、`persona_llm` 或 `DoubaoTtsClient`；收到小智 STT 显示用户字幕，`sentence_start` 显示回复字幕；停止或设备断线后迟到音频不进入 `PlayerCmd::Pcm`。

- [ ] **Step 2: 运行实时编排测试确认失败**

Run: `cargo test realtime_chat::tests --lib --quiet`

Expected: FAIL，因为小智分支尚未接入。

- [ ] **Step 3: 实现小智分支**

在会话启动后、现有 ASR 创建前选择引擎。小智模式不发送当前宠物 system prompt、问候语、历史或工具定义；仅显示当前宠物名称作为本地 HUD 标签。开始时显示“小智连接中”，成功后发送 `listen start` 并流式转发 P4 音频。小智 TTS 音频复用现有播放器；取消时先停止 P4 播放，再向小智发送 abort 并关闭连接。小智模式使用上游的可打断会话语义，但不会调用本地 ASR 验证打断。

- [ ] **Step 4: 运行实时编排测试确认通过**

Run: `cargo test realtime_chat::tests --lib --quiet`

Expected: PASS。

- [ ] **Step 5: Commit**

```bash
git add pc/src-tauri/src/realtime_chat.rs pc/src-tauri/src/voice_chat_settings.rs
git commit -m "feat: route realtime chat through xiaozhi"
```

### Task 6: 文档、回归、打包与真机验收

**Files:**
- Modify: `pc/docs/realtime-chat.md`
- Modify: `pc/.folder.md`
- Modify: `firmware/protocol.md`
- Modify: `pc/package.json`、`pc/package-lock.json`、`pc/src-tauri/Cargo.toml`、`pc/src-tauri/tauri.conf.json`、`pc/README.md`

**Interfaces:**
- Consumes: 任务 1–5 的设置、会话和状态接口。
- Produces: 可部署版本、操作说明和验收记录。

- [ ] **Step 1: 写用户路径与版本回归测试**

覆盖：小智未绑定时实时对话拒绝启动并显示绑定操作；绑定后无音频连接成功；API、OMLX 和小智切换后各自设置仍保留；USB 与 Wi-Fi 均走相同的小智会话入口。

- [ ] **Step 2: 运行完整静态与单元验证**

Run: `git diff --check && cargo test xiaozhi_protocol::tests --lib --quiet && cargo test xiaozhi_settings::tests --lib --quiet && cargo test xiaozhi_realtime::tests --lib --quiet && cargo test realtime_chat::tests --lib --quiet && cargo check --quiet && node --test src/ApiSettings.test.js && node scripts/desktop-license-inventory.test.mjs`

Expected: 全部通过；既有无关警告可记录但不得掩盖失败。

- [ ] **Step 3: 构建、签名并验证正式应用**

Run: `node scripts/build-development.mjs macos && codesign --verify --deep --strict --verbose=2 src-tauri/target/release/bundle/macos/HachimiDock.app`

Expected: 构建、许可证验证和签名验证均通过。

- [ ] **Step 4: 通过正式安装包真机验收**

先用客户端完成小智绑定；分别在 USB 和 Wi-Fi 下发起短句问答，验证用户字幕、回复字幕、完整语音、停止和切回 API/OMLX。记录阶段与耗时，不记录音频、转写、token 或身份。

- [ ] **Step 5: Commit**

```bash
git add pc/docs/realtime-chat.md pc/.folder.md firmware/protocol.md pc/package.json pc/package-lock.json pc/src-tauri/Cargo.toml pc/src-tauri/tauri.conf.json pc/README.md
git commit -m "docs: document xiaozhi realtime provider"
```

## Self-Review

- 规格中的固件不变、三条路径隔离、无 Agent/MCP、加密凭据、安全日志、USB/Wi-Fi 和明确失败状态分别由任务 2、3、5、6 覆盖。
- 任务 1 是明确门槛：在账号/OTA 服务未验证接受 PC 虚拟身份时，后续音频实现不得开始。
- 每个跨任务接口在对应任务的 Interfaces 区定义；小智不复用 `LlmConfig`，避免 API/OMLX 策略再次耦合。
- Review Focus 的五种输入/失败模式各有对应的测试任务。
- 计划保留 API/OMLX 源码边界；未把本地联网检索问题混入本项目。
