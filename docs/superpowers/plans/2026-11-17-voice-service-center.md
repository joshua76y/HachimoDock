# 语音与本地模型服务中心 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 ASR、TTS 和 LLM 具有独立、可验证的服务配置，并让 USB/Wi-Fi 使用相同的语音能力。

**Architecture:** 在 Tauri 端新增一个非秘密的统一服务偏好存储，运行时 provider 路由只读取它。现有云端密钥继续留在加密文件；Wi-Fi 音频通过已有识别器的同一 provider 路径；TTS 新增基于既有 `sherpa-onnx` 原生 Python 运行时的 Kokoro loopback worker，不使用 Docker；OMLX 仅执行 OpenAI 兼容健康检查。

**Tech Stack:** Rust/Tauri、React、现有 reqwest 与 Python 本地服务、Node 源码测试。

**Spec:** `docs/superpowers/specs/2026-11-17-voice-service-center-design.md`

## Global Constraints

- 不改变 ESP32-P4 固件协议、Wi-Fi 配对或设备端密钥边界。
- 云端凭据只能继续使用既有加密存储，不能写进新服务偏好 JSON。
- USB 与 Wi-Fi 对同一 ASR provider 必须输出相同的成功、空结果和失败语义。
- Kokoro 不可用时保留用户选择并给出可操作错误，不隐式回退到云端或 `say`；不得调用 Docker、依赖 Docker Desktop 或下载容器镜像。
- 不假定 OMLX 有稳定的第三方启动 CLI；仅以 OpenAI 兼容 API 健康检查判断就绪。

## Review Focus

- 旧 `~/.hachimo-local-asr.json` 的 `local` 选择迁移后重启仍选本地 ASR。
- UI 切换到火山 ASR 后，后端 provider 立即变为云端，不能残留 localStorage 的旧选择。
- Wi-Fi WAV 输入走云端 provider 时短音频、超时、空转写均能返回明确错误，不导致草稿卡住。
- Kokoro 未安装、端点不可达、返回非 WAV 时不会把损坏音频发往设备。
- OMLX 配置存在但 `/v1/models` 不可达时必须显示“未就绪”，而不是“已配置”。

### Task 1: 统一服务偏好持久化与 ASR 路由

**Files:**
- Create: `pc/src-tauri/src/voice_service_settings.rs`
- Modify: `pc/src-tauri/src/lib.rs`
- Modify: `pc/src-tauri/src/local_asr.rs`
- Modify: `pc/src-tauri/src/local_tts.rs`
- Modify: `pc/src-tauri/src/volcengine_asr.rs`
- Test: Rust unit tests in `voice_service_settings.rs`

**Interfaces:**
- Produces `VoiceServicePreferences`, `load_preferences()`, `save_preferences(input)`, `asr_provider()`, `tts_provider()` and a Tauri `voice_service_preferences` command pair.
- Consumes legacy `~/.hachimo-local-asr.json` only once as a migration source.

- [ ] **Step 1: Write failing migration and provider-selection tests**

Test an absent unified file migrating a legacy `provider=local, endpoint, engine, ttsProvider, localTtsVoice`; test that saving `provider=volcengine` changes `asr_provider()` to cloud and leaves cloud keys untouched.

- [ ] **Step 2: Run the focused test to verify it fails**

Run: `cargo test voice_service_settings --lib`
Expected: FAIL because the module and functions do not exist.

- [ ] **Step 3: Implement atomic unified preferences and routing accessors**

Store non-secret settings at the application data directory configured by `lib.rs`. Migrate legacy file values, validate provider IDs, atomically write via `NamedTempFile`, and change ASR/local TTS readers to use this module rather than reading home-directory JSON directly.

- [ ] **Step 4: Run focused and module tests**

Run: `cargo test voice_service_settings --lib && cargo test local_asr --lib && cargo test volcengine_asr --lib`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add pc/src-tauri/src/voice_service_settings.rs pc/src-tauri/src/lib.rs pc/src-tauri/src/local_asr.rs pc/src-tauri/src/local_tts.rs pc/src-tauri/src/volcengine_asr.rs
git commit -m "feat: unify voice service preferences"
```

### Task 2: 统一 Wi-Fi 与 USB 的 ASR provider

**Files:**
- Modify: `pc/src-tauri/src/device_voice_bridge.rs`
- Modify: `pc/src-tauri/src/volcengine_asr.rs`
- Test: `pc/src-tauri/src/device_voice_bridge.rs` unit tests
- Test: `firmware/tests/p4_voice_ptt_timeout_test.py`

**Interfaces:**
- Consumes `asr_provider()` from Task 1.
- Produces `transcribe_wav_using_active_provider(wav) -> Result<String, String>` with the same provider decision as `StreamingSpeechRecognizer`.

- [ ] **Step 1: Write failing tests for both selected providers**

Extract a provider-agnostic WAV dispatch seam. Test local dispatch is retained; test cloud dispatch delegates to the streaming recognizer bridge rather than returning the former “只认本地链路” error.

- [ ] **Step 2: Run focused tests to verify failure**

Run: `cargo test device_voice_bridge --lib`
Expected: FAIL because cloud WAV dispatch is not implemented.

- [ ] **Step 3: Implement shared WAV-to-recognizer bridge**

Validate/decode mono 16k WAV, feed chunks to `StreamingSpeechRecognizer`, finish it, await a final event with the existing bounded ASR timeout, and surface empty/error events faithfully. Keep all cloud credentials on PC.

- [ ] **Step 4: Run Rust and firmware bridge checks**

Run: `cargo test device_voice_bridge --lib && python3 firmware/tests/p4_voice_ptt_timeout_test.py`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add pc/src-tauri/src/device_voice_bridge.rs pc/src-tauri/src/volcengine_asr.rs firmware/tests/p4_voice_ptt_timeout_test.py
git commit -m "fix: use selected ASR provider for Wi-Fi PTT"
```

### Task 3: 本地 Kokoro TTS 服务与音频路由

**Files:**
- Create: `pc/src-tauri/src/kokoro_tts.rs`
- Create: `pc/resources/kokoro_server.py`
- Modify: `pc/src-tauri/src/doubao_tts.rs`
- Modify: `pc/src-tauri/src/lib.rs`
- Test: Rust unit tests in `kokoro_tts.rs` and `doubao_tts.rs`

**Interfaces:**
- Consumes `tts_provider()` and `KokoroPreferences` from Task 1.
- Produces `kokoro_tts::health()`, `ensure_service()`, `synthesize(text, voice, speed) -> TtsResult`, and Tauri status/test commands.

- [ ] **Step 1: Write failing Kokoro configuration and HTTP response tests**

Test a `kokoro` provider selects the new local backend; test unavailable `/health` reports not-ready; test a valid WAV response is accepted and non-WAV/empty response is rejected.

- [ ] **Step 2: Run focused tests to verify failure**

Run: `cargo test kokoro_tts --lib && cargo test doubao_tts --lib`
Expected: FAIL because Kokoro backend and provider route do not exist.

- [ ] **Step 3: Implement a managed native Kokoro service**

Reuse the existing `sherpa-onnx` Python runtime; never invoke Docker. On an explicit UI action, download the official `kokoro-multi-lang-v1_1` model archive into app data, validate its extracted model layout, and start a bundled loopback HTTP wrapper on port 8891. Poll `/health`, submit Chinese text and receive WAV. Convert/validate returned audio as 16k mono PCM before playback. Do not auto-download or auto-start during ordinary app startup.

- [ ] **Step 4: Route TTS by provider and preserve existing behavior**

Use `say` only for `macos`, new backend only for `kokoro`, and existing cloud client only for `volcengine`; keep cloud role-specific speaker/speed behavior unchanged and use the global Kokoro voice/speed for Kokoro.

- [ ] **Step 5: Run focused tests**

Run: `cargo test kokoro_tts --lib && cargo test doubao_tts --lib`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add pc/src-tauri/src/kokoro_tts.rs pc/resources/kokoro_server.py pc/src-tauri/src/doubao_tts.rs pc/src-tauri/src/lib.rs
git commit -m "feat: add managed local Kokoro TTS"
```

### Task 4: 服务状态与设置页改造

**Files:**
- Modify: `pc/src-tauri/src/voice_chat_settings.rs`
- Modify: `pc/src-tauri/src/realtime_chat.rs`
- Modify: `pc/src-tauri/src/lib.rs`
- Modify: `pc/src/ApiSettings.jsx`
- Modify: `pc/src/lib/api-configuration.js`
- Test: `pc/src/realtime-chat.test.js`
- Test: `pc/scripts/test-settings-layout-browser.mjs`

**Interfaces:**
- Consumes settings/status commands from Tasks 1 and 3 and existing `list_voice_chat_models`.
- Produces UI cards with actual ASR/TTS/LLM service state and provider-specific test actions.

- [ ] **Step 1: Write failing source and browser tests**

Assert settings no longer initialize ASR/TTS provider from localStorage; assert the view requests backend service preferences; assert OMLX calls `/v1/models` and displays distinct configured vs ready state; assert Kokoro option/status/test controls render.

- [ ] **Step 2: Run tests to verify failure**

Run: `node --test pc/src/realtime-chat.test.js && node pc/scripts/test-settings-layout-browser.mjs`
Expected: FAIL because the settings page still owns provider state and has no Kokoro/health controls.

- [ ] **Step 3: Implement backend status commands**

Expose independent ASR, TTS and LLM status. For OMLX/custom local OpenAI endpoints, call `/models` or `/v1/models` with a short timeout and return `configured`, `ready`, and `message` separately. Preserve saved endpoint/model/key even when unavailable.

- [ ] **Step 4: Implement UI from backend state**

Remove ASR/TTS provider reads/writes to localStorage. Save provider changes through Tauri and update state only after success. Add local ASR and Kokoro service controls, test buttons, migration cleanup, and status copy that distinguishes unavailable/configured/ready.

- [ ] **Step 5: Run tests and build**

Run: `node --test pc/src/realtime-chat.test.js pc/src/public-source-safety.test.js && node pc/scripts/test-settings-layout-browser.mjs && npm --prefix pc run build`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add pc/src-tauri/src/voice_chat_settings.rs pc/src-tauri/src/realtime_chat.rs pc/src-tauri/src/lib.rs pc/src/ApiSettings.jsx pc/src/lib/api-configuration.js pc/src/realtime-chat.test.js pc/scripts/test-settings-layout-browser.mjs
git commit -m "feat: show independent voice service status"
```

### Task 5: 回归验证、文档与设备测试

**Files:**
- Modify: `pc/.folder.md`
- Modify: `pc/docs/realtime-chat.md`
- Modify: `firmware/BUILD.md`

- [ ] **Step 1: Document the persisted settings, model requirements, and connection parity**

Describe provider selection, local model installation lifecycle, OMLX health semantics, security boundary, and USB/Wi-Fi timing differences.

- [ ] **Step 2: Run complete desktop and Rust suites**

Run: `npm --prefix pc test && cargo test --manifest-path pc/src-tauri/Cargo.toml`
Expected: PASS, or list existing unrelated failures separately.

- [ ] **Step 3: Build release artifacts**

Run: `npm --prefix pc run build && cargo build --manifest-path pc/src-tauri/Cargo.toml --release`
Expected: PASS.

- [ ] **Step 4: Perform manual acceptance with the connected board**

Verify USB/local ASR, USB/cloud ASR, Wi-Fi/local ASR, Wi-Fi/cloud ASR; test `say`, Kokoro and cloud TTS; stop OMLX and verify unavailable state, then restart it and verify ready state. Do not send agent text until the user confirms the target conversation.

- [ ] **Step 5: Commit**

```bash
git add pc/.folder.md pc/docs/realtime-chat.md firmware/BUILD.md
git commit -m "docs: describe voice service center"
```
