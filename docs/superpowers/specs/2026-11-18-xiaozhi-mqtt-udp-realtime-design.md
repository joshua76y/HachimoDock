# 小智官方 MQTT + UDP 实时问答设计

## 意图

将小智作为桌面端的第三种实时语音问答引擎，接入其当前官方服务所下发的 MQTT + 加密 UDP 通道。ESP32-P4 保持现有 16 kHz PCM 输入/输出协议，不刷写、不改动固件；PC 负责协议适配、Opus 和网络会话。

用户选择此方案的原因是希望得到官方小智的高质量、低延迟实时问答，同时继续保留既有 API 与本地 OMLX 两套独立链路。

## 已验证事实

- 官方默认 OTA 地址接受带标准身份头的最小 PC 虚拟设备请求；返回同时含 `mqtt`、`websocket` 和 `activation`。
- 官方当前 `Application::InitializeProtocol()` 在 MQTT 存在时优先选择 MQTT，而非 WebSocket。
- MQTT 负责 JSON 控制面；音频为 UDP 中的 AES-128-CTR 加密 Opus，包头 16 字节，序号单调递增；官方端下行通常是 24 kHz，P4 播放必须回到 16 kHz。

## 范围

### 本次实现

- 使用小智 OTA 获取、加密保存并刷新虚拟设备身份、MQTT 凭据和绑定状态。
- MQTT TLS 控制通道：连接、订阅/发布、hello、listen、abort、goodbye、STT/TTS 字幕事件。
- UDP 数据通道：服务端 hello 协商、AES-128-CTR、序号/重放检查、Opus 双向编解码、24 kHz → 16 kHz 下行重采样。
- 将其作为 `RealtimeEngine::Xiaozhi` 接入现有 P4 录音、屏幕字幕与播放器取消语义。
- 设置页提供“小智 AI”引擎选择、默认 OTA 地址、绑定状态/验证码和无音频连通性检查。

### 明确不做

- 不改 ESP32-P4 固件、USB/Wi-Fi 传输或既有 API/OMLX 设置、提示词、工具和对话策略。
- 不接 Agent、桌面派发、MCP、本地工具、智能家居、待办、音乐、网页搜索或服务端指令执行。
- 不在诊断、前端状态、日志或导出中保存/输出 PCM、转写、MQTT 密码、UDP 密钥/nonce、OTA token、完整虚拟身份或服务端原始响应。
- 不将“服务器返回 WebSocket”当成可绕过官方 MQTT 优先级的后门。

## 方案比较

1. **原 WebSocket 适配层**：实现较小，但官方默认响应已选择 MQTT，WebSocket 在当前官方客户端中不是首选路径；不采用。
2. **忽略 MQTT 强连官方 WebSocket**：表面改动最少，但违背官方运输选择，兼容性和长期稳定性不可验证；拒绝。
3. **原生 MQTT + UDP 桥接**：工作量较大，却是与当前官方运输一致的路径；采用。

## 架构

```text
P4 PCM 16 kHz <-> realtime_chat
                    |
              XiaozhiRealtime
                |           |
          MQTT/TLS       UDP/AES-CTR
          控制/字幕       Opus 音频
                \           /
                 小智官方服务
```

### 协议与状态

1. `xiaozhi_protocol` 将 OTA 响应解析为 `MqttProvisioning`，要求完整 MQTT 凭据；WebSocket 字段可被安全忽略但不使用。
2. `xiaozhi_settings` 将 OTA 地址、虚拟本地管理 MAC、UUID、MQTT 凭据和绑定挑战写入独立、加密的设置文件；前端只得到掩码、绑定阶段和短验证码。
3. `XiaozhiRealtime::connect` 建立 TLS MQTT 连接，使用 OTA 返还的 client ID/用户名/密码/主题。收到 MQTT hello 后，验证 session、UDP host/port、16 字节 hex key/nonce 和音频参数；随后才打开 UDP。
4. P4 每 20 ms 16 kHz PCM 先聚合为 60 ms，再编码 Opus。每个包建立 `type|flags|len|ssrc|timestamp|sequence` 头，以头作为 CTR IV 加密 payload 后发送。
5. 下行只接受当前 session、合法长度且严格递增序号的 UDP 包；解密、Opus 解码、重采样后才交给已有 `PlayerCmd::Pcm`。停止、断线、换引擎或过期 session 后丢弃所有迟到包。

### 依赖与打包

- MQTT：`rumqttc`，采用 Rustls TLS，不依赖用户机器安装 MQTT 库。
- CTR：`ctr` 与现有 `aes` 构成 AES-128-CTR。
- Opus：先做 macOS/Windows 打包编译探针；优先选择可随应用静态构建的实现。若 `opus`/`audiopus_sys` 无法在两端稳定打包，则在接入网络前停下改用纯 Rust Opus 实现，而不要求用户安装 `libopus`。
- 所有新增依赖在许可证清单中固定并做打包前检查。

## 失败与恢复

- OTA 未绑定：显示绑定码，禁止 MQTT、UDP 和音频。
- MQTT 认证、TLS 或连接失败：显示可操作的连接失败状态；不会回退为 API/OMLX，也不泄露凭据。
- hello、UDP 密钥或协议参数不合法：关闭 MQTT/UDP，显示“服务协议不兼容”。
- UDP 不可达、超时、解密失败或重放包：立即停止该实时会话；下次由用户重新开始，重新走 hello，不暗中复用旧密钥。
- MQTT 重连只在尚未进入音频会话时允许；活动会话断线直接结束，避免向错误 session 发送音频。

## 验收

1. 可在设置页获得绑定码并在绑定后显示“可用”，不暴露密钥或身份。
2. 小智模式下 P4 语音可显示用户/回复字幕、完整播放服务端语音；取消和断线时不播放迟到音频。
3. API 和 OMLX 分支仍经原 ASR → LLM → TTS 路径，且小智网络请求不包含它们的 Key、模型或提示词。
4. USB 与 Wi-Fi 的 P4 音频路径功能一致；各自仅使用现有的 PC-P4 传输。
