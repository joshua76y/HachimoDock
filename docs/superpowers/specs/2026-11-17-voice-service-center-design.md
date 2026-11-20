# 语音与本地模型服务中心设计

## 意图

将哈基米机的 ASR、TTS 与对话 LLM 解耦：每项服务各自选择云端或本地实现，桌面端成为唯一配置与状态入口。设备经 USB 或 Wi-Fi 连接时必须使用同一项服务选择与同一套能力，不能因传输方式被静默降级。

用户需要在不运行终端命令的情况下，看到本地服务是否可用、启动或停止本地 ASR，并使用效果好但轻量的离线中文 TTS。现有 OMLX 保持 OpenAI 兼容 LLM 后端，不强行重新托管其模型。

## 范围

### 目标

- 后端配置文件成为 ASR、TTS、LLM 选择的唯一事实来源；旧浏览器 localStorage 只做一次迁移，不再决定运行时。
- ASR 可独立选择火山或本地 SenseVoice/Qwen3；USB 与 Wi-Fi PTT 均遵循该选择。
- TTS 可独立选择火山、macOS `say`、本地 Kokoro 中文 82M；Kokoro 复用本机的 `sherpa-onnx` 原生运行时，由桌面端按需启动、检测、测试，绝不要求 Docker。
- LLM 保持现有云端/O​​MLX/OpenAI 兼容配置；UI 以实际健康检查结果呈现就绪状态，而非只根据默认 token 认定可用。
- 设置页展示每项服务的配置、运行状态、测试入口及可读错误，且保存后的结果立即反映实际运行链路。

### 非目标

- 不把 OMLX、Kokoro 或其模型权重打包进应用安装包；模型下载独立、可见、可恢复。
- 不承诺本地 LLM 的工具调用、联网搜索、智能家居或桌面 Agent 调度；本地对话先维持短上下文聊天边界。
- 不新增设备端密钥保存，也不改变设备 USB/Wi-Fi 配对与身份绑定机制。
- 不在本轮实现每个角色独立的 Kokoro 音色映射；先提供一个全局本地 Kokoro 音色，角色云端音色仍仅在火山 TTS 时生效。

## 现状与根因

`ApiSettings.jsx` 的 ASR/TTS 单选项存于浏览器 `localStorage`，而运行链路读 `~/.hachimo-local-asr.json`。因此 UI 显示与实际使用的 provider 可不一致。Wi-Fi 的 `device_voice_bridge` 只允许本地 ASR；USB 的 `StreamingSpeechRecognizer` 则可按后端配置分流。本地 TTS 只有 macOS `say`，并绕过角色语速和云端角色音色。LLM 的 `omlx` preset 已符合 OpenAI 兼容协议，但状态页没有探测服务。

## 方案比较

1. **继续修补 localStorage 和现有 JSON 文件**：改动最小，但两份状态仍会再次漂移，无法可靠表达服务状态；不采用。
2. **每项服务各自新增独立配置文件与独立 UI 状态**：可用，但迁移和跨服务状态容易重复，设置页也难以理解；不采用。
3. **后端统一服务偏好与状态接口（采用）**：Rust 持有唯一 JSON 配置与 provider 路由；React 仅读取、编辑和显示命令返回值。USB/Wi-Fi 共同调用识别抽象。该方案改动集中、可测试，能平滑迁移旧文件。

本地 TTS 的选择：

- macOS `say`：零下载、启动快，但中文自然度及角色感有限；保留。
- Qwen3-TTS 0.6B：中文表现和可控性更强，但模型、Python/Torch 依赖和内存占用不符合本项目“轻量、随客户端可用”的首选；暂不纳入第一版。
- **Kokoro 中文 82M（采用）**：体量低、支持中文和多音色，可由 `sherpa-onnx` 的 Python 原生接口直接推理；作为“本地轻量模型”选项。模型包本身仍可能达到数百 MB（含多音色、词典与中文前端），但不会带来 Docker 守护进程、容器镜像或第二套推理框架。

## 架构

```text
ApiSettings
    │  Tauri commands
    ▼
VoiceServiceSettings (唯一持久化配置)
    ├─ ASR: volcengine | local(endpoint, engine)
    ├─ TTS: volcengine | macos(voice) | kokoro(endpoint, voice)
    └─ LLM: cloud | omlx/custom(endpoint, model, secret)
    │
    ├─ ServiceStatus/health checks
    ├─ Local ASR lifecycle
    └─ Kokoro lifecycle（按需下载模型，启动 loopback 原生 worker）

USB PTT ─┐
Wi-Fi PTT ├── StreamingSpeechRecognizer ── ASR provider
          │
Realtime chat ── LLM provider ── TTS provider ── device audio
```

Wi-Fi PTT 上传的是完整 WAV：桥接层把 WAV 解码为 PCM 并馈入与 USB 相同的 `StreamingSpeechRecognizer`，等待最终转写结果。因此火山与本地 ASR 的选择、错误文本和确认草稿行为一致；它只多出一个完整录音的上传时间。

TTS 的服务语义不同：本地 `say` 不需要常驻进程，其“状态”是系统命令和音色探测结果；Kokoro 与本地 ASR 都是桌面端托管的 loopback worker，可停止并在闲置时退出，不依赖 Docker。首次下载和模型冷加载允许分钟级等待；常规探活仍保持短超时，避免设置页无响应。LLM 不强制启动 OMLX：先健康探测并引导用户启动已安装的 OMLX，避免假设第三方应用有稳定的启动/模型加载 CLI。

## 设置页交互

每个服务卡片独立呈现：

```text
语音合成                         状态：Kokoro 已就绪
( ) 火山引擎  ( ) macOS 系统语音  (●) Kokoro 本地轻量
音色：[中文女声 01 v]  [测试播放]
模型：[下载并安装]  服务：[启动/停止]  地址：http://127.0.0.1:8891
```

- 切换 provider 时立即调用后端保存命令；写入成功后再更新 UI。
- 旧 localStorage 仅在首次读取时迁移，后端成功保存后删除旧键。
- 云端 provider 没有 key 时展示“未配置”，本地服务未运行时展示“未启动/不可达”，不能显示“已配置”冒充“已就绪”。
- LLM 选择 OMLX 时探测 `/v1/models`；成功才显示“本地 OMLX 已就绪”。失败时保留配置，提示用户启动 OMLX，不影响 ASR/TTS。

## 数据与迁移

在应用配置目录新增 `voice-service-settings.json`，写入采用临时文件后 rename，避免进程意外退出导致半个 JSON。首次加载按以下优先级迁移：新的统一配置、旧 `~/.hachimo-local-asr.json`、浏览器传入的旧偏好、默认值。云端密钥继续使用现有加密存储，不复制到新 JSON。

## 验收标准

- 选择本地/火山 ASR 后，重启桌面端，USB 与 Wi-Fi PTT 都调用相同的 provider。
- Wi-Fi + 火山 ASR 成功生成待确认草稿；Wi-Fi + 本地 ASR 保持现有能力。
- 选择 `say`、Kokoro、火山 TTS 的测试播放可明确成功或给出可操作错误；Kokoro 未安装/未启动绝不伪装为可用。
- 选择 OMLX 且本地端点不可达时，设置页显示未就绪；端点恢复后无需重填即可变为已就绪。
- 旧用户现有本地 ASR、TTS 和云端密钥配置升级后不丢失。

## 风险与控制

- Kokoro 的模型下载会受网络影响：仅在用户点击“下载并安装”后下载官方模型包；运行期复用既有 `sherpa-onnx`，将安装、下载、启动分开显示，失败不影响 `say` 和云端 TTS。绝不要求安装 Docker。
- Wi-Fi 火山 ASR 比 USB 多一次 WAV 上传，长录音的最终结果延迟会更高：保持录音上限与超时提示，且不在设备端保存 key。
- OMLX 的启动控制接口尚未作为稳定依赖：第一版只做兼容端点健康检查，避免误杀或错误启动用户其他本地模型服务。
