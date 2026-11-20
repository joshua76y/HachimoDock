# 小智官方 MQTT + UDP 实时问答 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 不改 ESP32-P4 固件，将小智官方 MQTT + 加密 UDP 接入为第三种实时问答引擎。

**Architecture:** 小智独立为 `RealtimeEngine::Xiaozhi`：OTA 供应保存 MQTT 凭据，TLS MQTT 传控制/字幕，AES-CTR UDP 传 Opus。`realtime_chat` 仅在小智引擎被选中时走此路径，API/OMLX 留在既有 ASR → LLM → TTS 管线。

**Tech Stack:** Rust/Tauri 2、`rumqttc`、现有 `aes` + `ctr`、经打包探针验证的 Opus、React、`secure_config`。

**Spec:** `docs/superpowers/specs/2026-11-18-xiaozhi-mqtt-udp-realtime-design.md`

## Global Constraints

- 不修改 P4 固件及 USB/Wi-Fi 传输；不接入 Agent、MCP、本地工具、搜索或家庭控制。
- 不记录或输出 PCM、转写、OTA token、MQTT 密码、UDP key/nonce、完整虚拟身份或服务端原文。
- 未绑定、未取得合法 MQTT hello 或会话停止后，不得发送或播放音频。
- UDP 入站必须验证当前 session、包长、包型和严格递增序号，并在 AES-CTR 解密后再解码。
- 新依赖必须通过 macOS/Windows 打包探针及许可证清单；若 Opus 不能随应用构建，停在依赖探针改用纯 Rust 实现。

## Review Focus

- OTA 同时给 MQTT/WebSocket：只采用 MQTT，测试归属任务 1。
- 未绑定或 MQTT 字段不全：禁止网络会话，测试归属任务 1。
- 伪造、乱序、重放 UDP：不得产生 PCM，测试归属任务 4/5。
- 24 kHz 下行音频必须变为 16 kHz PCM，测试归属任务 4。
- 取消、断线、换引擎后的迟到音频不得进入播放器，测试归属任务 5。

---

### Task 1: 官方 MQTT OTA 供应协议

**Files:**
- Modify: `pc/src-tauri/src/xiaozhi_protocol.rs`
- Test: `pc/src-tauri/src/xiaozhi_protocol.rs`

**Interfaces:**
- Produces: `MqttProvisioning`、`OtaConfig::mqtt_provisioning() -> Result<MqttProvisioning, XiaozhiProtocolError>`、已有 `OtaRequest::http_headers()`。
- Consumes: `serde_json::Value`；不访问 P4、ASR、LLM/TTS。

- [ ] **Step 1: 写失败测试**

覆盖 mixed MQTT/WebSocket 时选 MQTT；`activation`、缺 `endpoint/client_id/username/password/publish_topic`、非法 endpoint/keepalive 都无法取得供应；错误和 `Debug` 均不能包含凭据。

- [ ] **Step 2: 验证测试失败**

Run: `cargo test xiaozhi_protocol::tests --lib --quiet`

Expected: FAIL，因为现有解析器拒绝 mixed 响应。

- [ ] **Step 3: 实现纯 `MqttProvisioning` 解析**

供应字段私有且脱敏；endpoint 仅允许主机加可选端口，keepalive 限于合理范围。存在 `activation` 时 `mqtt_provisioning()` 必须返回绑定错误，绝不读取/使用 WebSocket token。

- [ ] **Step 4: 验证离线与显式 OTA 探测**

Run: `cargo test xiaozhi_protocol::tests --lib --quiet && XIAOZHI_LIVE_TEST=1 XIAOZHI_OTA_URL='https://api.tenclass.net/xiaozhi/ota/' cargo test xiaozhi_protocol::tests::live_ota_smoke --lib -- --ignored --nocapture`

Expected: PASS；真实探测只输出状态和耗时，不建立 MQTT/UDP。

- [ ] **Step 5: Commit**

`git commit -m "feat: parse xiaozhi mqtt provisioning" -- pc/src-tauri/src/xiaozhi_protocol.rs`

### Task 2: 独立加密设置与实时引擎枚举

**Files:**
- Create: `pc/src-tauri/src/xiaozhi_settings.rs`
- Modify: `pc/src-tauri/src/voice_chat_settings.rs`
- Modify: `pc/src-tauri/src/secure_config.rs`
- Modify: `pc/src-tauri/src/lib.rs`
- Test: `pc/src-tauri/src/xiaozhi_settings.rs`、`pc/src-tauri/src/voice_chat_settings.rs`

**Interfaces:**
- Consumes: `OtaRequest`、`OtaConfig`、`MqttProvisioning`。
- Produces: 无秘密的 `XiaozhiStatus`、`save_xiaozhi_settings`、`activate_xiaozhi`、`test_xiaozhi_connection`、`resolved_mqtt_provisioning()`、`RealtimeEngine::{LlmPipeline, Xiaozhi}`。

- [ ] **Step 1: 写失败测试**

断言旧设置默认 `LlmPipeline`；切换引擎不改 LLM/ASR/TTS；小智独立文件被加密并被 macOS 防丢钥扫描；序列化状态没有 MQTT/UDP 秘密和完整身份。

- [ ] **Step 2: 验证测试失败**

Run: `cargo test 'xiaozhi_settings::tests|voice_chat_settings::tests|secure_config::tests' --lib --quiet`

Expected: FAIL，因为小智设置与枚举尚不存在。

- [ ] **Step 3: 实现加密设置和 Tauri 命令**

`xiaozhi-settings.json` 用 `xiaozhi-realtime` purpose 保存 OTA URL、本地管理 MAC/UUID、MQTT 供应和绑定挑战；`test_xiaozhi_connection` 仅认证 MQTT 后即断开，绝不开 UDP/音频。

- [ ] **Step 4: 验证并提交**

Run: `cargo test 'xiaozhi_settings::tests|voice_chat_settings::tests|secure_config::tests' --lib --quiet`

Expected: PASS。

Commit only touched hunks with message `feat: add isolated xiaozhi mqtt settings`。

### Task 3: 小智设置页

**Files:**
- Modify: `pc/src/ApiSettings.jsx`
- Modify: `pc/src/lib/api-configuration.js`
- Modify: `pc/src/ApiSettings.test.js`

**Interfaces:**
- Consumes: Task 2 的 Tauri 命令与 `RealtimeEngine` 状态。
- Produces: “模型链路 / 小智 AI”选择、绑定和无音频 MQTT 测试 UI。

- [ ] **Step 1: 写失败测试**

断言小智不进入 `LLM_PRESETS`，不会渲染/提交 OpenAI 模型或 Key，且切回时保留 LLM 草稿。

- [ ] **Step 2: 验证测试失败**

Run: `node --test src/ApiSettings.test.js`

Expected: FAIL，因为页面尚无小智状态和命令。

- [ ] **Step 3: 实现引擎卡**

在“对话大模型”前增加引擎选择；小智卡默认官方 OTA 地址，提供保存/获取绑定状态、绑定后重试和 MQTT 测试。只渲染绑定码、掩码和分类状态。

- [ ] **Step 4: 验证并提交**

Run: `node --test src/ApiSettings.test.js`

Expected: PASS。

Commit only touched hunks with message `feat: configure xiaozhi realtime engine`。

### Task 4: 打包探针、Opus 与 AES-CTR UDP 原语

**Files:**
- Create: `pc/src-tauri/src/xiaozhi_audio.rs`
- Modify: `pc/src-tauri/Cargo.toml`
- Modify: `pc/src-tauri/Cargo.lock`
- Modify: `licenses/DESKTOP-RUNTIME-DEPENDENCIES.json`
- Modify: `pc/scripts/dependency-license-resources.mjs`
- Test: `pc/src-tauri/src/xiaozhi_audio.rs`

**Interfaces:**
- Produces: `PcmToOpus`、`OpusToPcm`、`resample_to_16k`、`UdpPacket`、`encrypt_udp_payload`、`decrypt_udp_payload`。
- Consumes: 16 kHz PCM、16-byte key/nonce；不建立网络。

- [ ] **Step 1: 写失败测试与固定依赖**

加入 `rumqttc` 0.25.1 Rustls TLS、`ctr` 0.10.1，选择可静态打包 Opus。测试三帧 20 ms PCM 聚合为 60 ms Opus、24→16 kHz、CTR 往返、篡改包长/型/序号拒绝。

- [ ] **Step 2: 验证测试失败**

Run: `cargo test xiaozhi_audio::tests --lib --quiet`

Expected: FAIL，因为模块/依赖不存在。

- [ ] **Step 3: 实现原语**

包头严格为网络序 `0x01|0|len|ssrc|timestamp|sequence`；CTR IV 只覆写官方 nonce 的 offset 2、8、12。先验长度后再解密/重采样，错误不携带秘密。

- [ ] **Step 4: 验证许可证与打包前置条件**

Run: `node -e "import('./scripts/dependency-license-resources.mjs').then(m => console.log(JSON.stringify(m.verifyDesktopLicenseInventory('..'))))" && node scripts/desktop-license-inventory.test.mjs && cargo test xiaozhi_audio::tests --lib --quiet`

Expected: PASS。Opus 无法在 macOS/Windows 静态构建则停止并改用纯 Rust 实现，不进入任务 5。

- [ ] **Step 5: Commit**

Commit only touched hunks with message `feat: add xiaozhi audio transport primitives`。

### Task 5: MQTT + UDP 会话和实时编排

**Files:**
- Create: `pc/src-tauri/src/xiaozhi_realtime.rs`
- Modify: `pc/src-tauri/src/realtime_chat.rs`
- Modify: `pc/src-tauri/src/voice_chat_settings.rs`
- Modify: `pc/src-tauri/src/lib.rs`
- Test: `pc/src-tauri/src/xiaozhi_realtime.rs`、`pc/src-tauri/src/realtime_chat.rs`

**Interfaces:**
- Consumes: `resolved_mqtt_provisioning()`、Task 4 原语、`RealtimeEngine`。
- Produces: `XiaozhiRealtime::{connect,start_listening,push_pcm_16k,stop_listening,next_event,abort,close}` 和 `run_xiaozhi_session(...)`。

- [ ] **Step 1: 写失败测试**

用伪控制传输/UDP socket 断言：MQTT 连接后才 hello；错误 session、未知 JSON、MCP/system/custom、非法 hello、重复 UDP 与关闭后 UDP 不产生 PCM；取消/断线后 `PlayerCmd::Pcm` 不会收到迟到音频；小智不创建 ASR、LLM 或豆包 TTS。

- [ ] **Step 2: 验证测试失败**

Run: `cargo test 'xiaozhi_realtime::tests|realtime_chat::tests' --lib --quiet`

Expected: FAIL，因为小智会话和分派尚不存在。

- [ ] **Step 3: 实现有界会话与分派**

`rumqttc` TLS 仅使用 OTA 返回的 MQTT 供应；有效 hello 后才建 UDP。小智模式流转 P4 PCM、STT/字幕和回复 PCM，HUD 显示“小智连接中”；停止先 Flush P4 再 abort/close。原 `run_session` 的 API/OMLX 语义不变。

- [ ] **Step 4: 验证并提交**

Run: `cargo test 'xiaozhi_realtime::tests|realtime_chat::tests' --lib --quiet`

Expected: PASS。

Commit only touched hunks with message `feat: route realtime chat through xiaozhi mqtt`。

### Task 6: 文档、构建与真机验收

**Files:**
- Modify: `pc/docs/realtime-chat.md`
- Modify: `pc/.folder.md`
- Modify: `firmware/protocol.md`
- Modify: `docs/superpowers/specs/2026-11-17-xiaozhi-realtime-provider-design.md`

**Interfaces:**
- Consumes: Tasks 1–5。
- Produces: 配置/网络故障说明与小智 MQTT/UDP 边界文档。

- [ ] **Step 1: 记录边界**

说明绑定、MQTT/UDP 网络要求、UDP 被 NAT/防火墙拦截时的提示，标注旧 WebSocket-only 设计被取代且 API/OMLX 不受影响。

- [ ] **Step 2: 全量验证**

Run: `cargo test --lib --quiet && node --test src/ApiSettings.test.js && npm run build && cargo tauri build --debug`

Expected: 测试和桌面构建通过；只报告既有警告。

- [ ] **Step 3: 真机验收与提交**

验证绑定、MQTT 测试、P4 USB/Wi-Fi 的录音-取消-完整回复，以及 API/OMLX 回归；UDP 不可达必须明确失败且不能播放半段音频。提交仅本任务改动，消息 `docs: document xiaozhi mqtt realtime mode`。

## Self-review

- OTA、设置、UI、打包依赖、Opus/CTR、MQTT/UDP、编排、文档和真机验收分别由任务 1–6 覆盖。
- 任务接口从 OTA → 设置 → 原语 → 会话/编排单向消费，名称一致。
- 当前工作树含大量已有未提交改动；每次提交只能选择本计划对应 hunks，绝不做宽泛暂存。
