/**
 * [Input] Shared avatar provider configs including persisted video parameters, native Volcengine ASR credential commands, and optional return navigation.
 * [Output] Independent LLM/ASR reads, honest credential-save errors and retained navigation drafts; saved ASR changes are broadcast with save vs. test results distinguished.
 * [Pos] top-level page node in pc/src
 * [Sync] If this file changes, update this header and `pc/src/.folder.md`.
 */

import React, { useEffect, useMemo, useState } from "react";
import PromptSounds from "./PromptSounds.jsx";
import { invoke } from "@tauri-apps/api/core";
import {
  AlertCircle,
  ArrowLeft,
  CheckCircle2,
  Eye,
  EyeOff,
  ExternalLink,
  KeyRound,
  Loader2,
  ShieldCheck,
  Sparkles,
} from "lucide-react";
import PageShell from "./shell/PageShell.jsx";
import Card from "./shell/Card.jsx";

/* 后端 local_tts_voices 返回 [{name, available}]（available=false = 探测时无声）。
 * 下拉里只显示可发声的音色；但当前已保存的音色即使不可用也保留，
 * 否则 select 的 value 会落空、用户选择被悄悄改写。 */
function usableTtsVoices(voices, currentVoice) {
  const names = voices
    .filter((item) => item && typeof item.name === "string" && item.available !== false)
    .map((item) => item.name);
  if (currentVoice && !names.includes(currentVoice)) names.unshift(currentVoice);
  return names;
}

import LlmNetworkSettings from "./LlmNetworkSettings.jsx";
import GenerationDiagnostics from "./GenerationDiagnostics.jsx";
import TextModelSelect from "./TextModelSelect.jsx";
import ImageModelSelect from "./ImageModelSelect.jsx";
import { VideoModelSelect } from "./VideoGenerationSettings.jsx";
import { useUsageHelp } from "./shell/DeviceContext.jsx";
import UsageHelp from "./UsageHelp.jsx";
import { useExitProtection } from "./lib/exit-protection.js";
import {
  VIDEO_PROVIDERS,
  loadProviderConfig,
  saveProviderConfig,
} from "./lib/avatar-pipeline/provider-config.js";
import {
  LLM_PRESETS,
  DEFAULT_LLM_PROVIDER,
  describeLlmPreset,
  selectLlmProvider,
  changeLlmEndpoint,
  emitApiConfigurationUpdated,
  providerCredentialsConfigured,
} from "./lib/api-configuration.js";

function loadVideoConfigs() {
  return Object.fromEntries(
    VIDEO_PROVIDERS.map((provider) => [provider.id, loadProviderConfig(provider.id)]),
  );
}
function persistedProviderConfig(config) {
  return {
    apiKey: config.apiKey || "",
    accessKey: config.accessKey || "",
    secretKey: config.secretKey || "",
    baseUrl: config.baseUrl || "",
    model: config.model || "",
    thinkingModel: config.thinkingModel || "",
    imageModel: (config.imageModel || "").trim(),
    fastGeneration: config.fastGeneration !== false,
    videoParameters: config.videoParameters || {},
    advanced: config.advanced || {},
  };
}

function resultToneClass(tone) {
  return tone ? ` is-${tone}` : "";
}

/* 单例 <audio>：新试听先停掉上一段，一次点击只播一次。
 * 之前每次点击都 new Audio，旧播放不会停，快速点几次就叠好几层声音。 */
let ttsPreviewAudio = null;
function playTtsPreviewWav(base64) {
  return new Promise((resolve, reject) => {
    if (ttsPreviewAudio) {
      try { ttsPreviewAudio.pause(); } catch { /* ignore */ }
      ttsPreviewAudio = null;
    }
    const audio = new Audio(`data:audio/wav;base64,${base64}`);
    audio.loop = false;
    ttsPreviewAudio = audio;
    const cleanup = () => { if (ttsPreviewAudio === audio) ttsPreviewAudio = null; };
    audio.onended = () => { cleanup(); resolve(); };
    audio.onerror = () => { cleanup(); reject(new Error("音频解码/播放失败")); };
    audio.play().then(() => { /* 已开始播放，等 onended */ }).catch((error) => {
      cleanup();
      reject(error);
    });
  });
}

export default function ApiSettings({ onBack, configReadError = "", active = true }) {
  const usageHelp = useUsageHelp();
  const [showSecrets, setShowSecrets] = useState(false);
  const [videoConfigs, setVideoConfigs] = useState(loadVideoConfigs);
  const [savedVideoConfigs, setSavedVideoConfigs] = useState(() => videoConfigs);
  const [providerResults, setProviderResults] = useState({});
  const [providerPending, setProviderPending] = useState({});
  const [savedLlm, setSavedLlm] = useState(null);
  const [asrApiKey, setAsrApiKey] = useState("");
  /* 本地 ASR 与火山引擎并列。初始值只用于首帧，随后由后端统一配置覆盖。 */
  const [asrProvider, setAsrProvider] = useState("volcengine");
  const [localAsrEndpoint, setLocalAsrEndpoint] = useState("http://127.0.0.1:8900");
  const [localAsrEngine, setLocalAsrEngine] = useState("sensevoice");
  const [localAsrProbe, setLocalAsrProbe] = useState({ pending: false, message: "" });
  /* 语音合成来源：火山云端 / 本地 macOS 语音（say），与 ASR 解耦，各配各的。 */
  const [ttsProvider, setTtsProvider] = useState("volcengine");
  const [ttsVoice, setTtsVoice] = useState("Tingting");
  const [ttsVoices, setTtsVoices] = useState([]);
  const [ttsVoicesPending, setTtsVoicesPending] = useState(true);
  const [ttsProbe, setTtsProbe] = useState({ pending: false, message: "" });
  const [kokoroState, setKokoroState] = useState({ pending: false, message: "" });
  const [kokoroVoice, setKokoroVoice] = useState("zf_001");

  useEffect(() => {
    let alive = true;
    setTtsVoicesPending(true);
    invoke("local_tts_voices")
      .then((list) => {
        if (alive) setTtsVoices(Array.isArray(list) ? list : []);
      })
      .catch(() => {})
      .finally(() => {
        if (alive) setTtsVoicesPending(false);
      });
    return () => {
      alive = false;
    };
  }, []);

  /* 运行期 provider 只以 Tauri 后端为准。localStorage 仅保留为旧版迁移来源，
   * 不能再在重启后把界面切回与实际识别链路不同的状态。 */
  useEffect(() => {
    let alive = true;
    invoke("voice_service_preferences").then((preferences) => {
      if (!alive || !preferences) return;
      const asr = preferences.asr || {};
      const tts = preferences.tts || {};
      setAsrProvider(asr.provider === "local" ? "local" : "volcengine");
      setLocalAsrEndpoint(asr.endpoint || "http://127.0.0.1:8900");
      setLocalAsrEngine(asr.engine || "sensevoice");
      setTtsProvider(tts.provider === "macos" ? "local" : (tts.provider || "volcengine"));
      setTtsVoice(tts.macosVoice || "Tingting");
      setKokoroVoice(tts.kokoroVoice || "zf_001");
    }).catch(() => {});
    return () => { alive = false; };
  }, []);

  const saveTtsPreferences = async (provider, nextMacosVoice = ttsVoice, nextKokoroVoice = kokoroVoice) => {
    return invoke("save_voice_service_preferences", {
      input: {
        asrProvider: null,
        localAsrEndpoint: null,
        localAsrEngine: null,
        ttsProvider: provider === "local" ? "macos" : provider,
        macosTtsVoice: nextMacosVoice,
        kokoroEndpoint: null,
        kokoroVoice: nextKokoroVoice,
      },
    });
  };

  const chooseTtsProvider = (provider) => {
    setTtsProvider(provider);
    saveTtsPreferences(provider).catch((error) => {
      setTtsProbe({ pending: false, message: "保存语音合成来源失败：" + String(error) });
    });
  };

  useEffect(() => {
    if (ttsProvider !== "kokoro") return undefined;
    let alive = true;
    invoke("kokoro_tts_status")
      .then((status) => {
        if (alive) setKokoroState({ pending: false, message: status?.message || "Kokoro 状态未知" });
      })
      .catch((error) => {
        if (alive) setKokoroState({ pending: false, message: "读取 Kokoro 状态失败：" + String(error) });
      });
    return () => { alive = false; };
  }, [ttsProvider]);

  const [asrState, setAsrState] = useState({
    loading: true,
    pending: false,
    configured: false,
    tone: "muted",
    message: "正在读取语音识别配置…",
  });
  // 实时对话：服务预填默认模型并允许修改；豆包 Seed TTS 2.0 复用识别 Key。
  const [voiceChat, setVoiceChat] = useState({
    loading: true,
    llmPending: false,
    realtimeEngine: "llm_pipeline",
    llmConfigured: false,
    llmProvider: DEFAULT_LLM_PROVIDER,
    llmBaseUrl: "",
    llmModel: LLM_PRESETS.find(preset => preset.id === DEFAULT_LLM_PROVIDER).model,
    llmWebSearch: true,
    llmApiKey: "",
    llmApiKeyMasked: "",
    llmApiKeyMasks: {},
    llmCustomBaseUrl: "",
    llmTone: "muted",
    llmMessage: "正在读取对话大模型配置…",
  });
  const [xiaozhi, setXiaozhi] = useState({
    loading: true,
    pending: false,
    otaUrl: "https://api.tenclass.net/xiaozhi/ota/",
    configured: false,
    activationState: "unconfigured",
    activationCode: null,
    deviceIdMasked: "",
    mcpEnabled: false,
    tone: "muted",
    message: "正在读取小智连接配置…",
  });
  const llmDraft = JSON.stringify([voiceChat.llmProvider, voiceChat.llmBaseUrl, voiceChat.llmModel,
    voiceChat.llmWebSearch, voiceChat.llmApiKey]);
  useEffect(() => { if (!voiceChat.loading && savedLlm === null) setSavedLlm(llmDraft); }, [voiceChat.loading, savedLlm, llmDraft]);
  const dirty = JSON.stringify(videoConfigs) !== JSON.stringify(savedVideoConfigs)
    || !!asrApiKey || (savedLlm !== null && savedLlm !== llmDraft);
  const saving = voiceChat.llmPending || xiaozhi.pending || asrState.pending || Object.values(providerPending).some(Boolean);
  useExitProtection(saving ? "API 配置正在保存或测试" : dirty ? "API 配置有未保存的修改" : "");

  const applyVoiceChatStatus = (status, patch = {}) => {
    setVoiceChat((current) => ({
      ...current,
      ...patch,
      loading: false,
      realtimeEngine: status?.realtimeEngine || current.realtimeEngine,
      llmConfigured: status?.llmConfigured === true,
      llmWebSearch: status?.llmWebSearch !== false,
      llmProvider: status?.llmProvider || current.llmProvider,
      llmBaseUrl: status?.llmBaseUrl || "",
      llmModel: status?.llmModel || "",
      llmApiKeyMasked: status?.llmApiKeyMasked || "",
      llmApiKeyMasks: status?.llmApiKeyMasks || {},
      llmCustomBaseUrl: status?.llmCustomBaseUrl || "",
    }));
  };

  const applyXiaozhiStatus = (status, patch = {}) => {
    setXiaozhi((current) => ({
      ...current, ...patch, loading: false,
      otaUrl: status?.otaUrl || current.otaUrl,
      configured: status?.configured === true,
      activationState: status?.activationState || "unconfigured",
      activationCode: status?.activationCode || null,
      deviceIdMasked: status?.deviceIdMasked || "",
      mcpEnabled: status?.mcpEnabled === true,
    }));
  };

  const selectRealtimeEngine = async (realtimeEngine) => {
    if (voiceChat.realtimeEngine === realtimeEngine || voiceChat.llmPending) return;
    setVoiceChat((current) => ({ ...current, llmPending: true }));
    try {
      const status = await invoke("save_voice_chat_settings", { input: { realtimeEngine } });
      applyVoiceChatStatus(status, { llmPending: false });
    } catch (error) {
      setVoiceChat((current) => ({ ...current, llmPending: false, llmTone: "error", llmMessage: `切换实时引擎失败：${String(error)}` }));
    }
  };

  const activateXiaozhi = async () => {
    setXiaozhi((current) => ({ ...current, pending: true, tone: "muted", message: "正在获取小智绑定状态…" }));
    try {
      await invoke("save_xiaozhi_settings", { input: { otaUrl: xiaozhi.otaUrl, mcpEnabled: xiaozhi.mcpEnabled } });
      const status = await invoke("activate_xiaozhi");
      applyXiaozhiStatus(status, {
        pending: false,
        tone: status?.configured ? "success" : "warning",
        message: status?.configured ? "小智已绑定，可进行 MQTT 连接测试。" : "请按绑定码完成小智绑定后，再点击重新获取状态。",
      });
    } catch (error) {
      setXiaozhi((current) => ({ ...current, pending: false, tone: "error", message: `获取小智状态失败：${String(error)}` }));
    }
  };

  const toggleXiaozhiMcp = async (mcpEnabled) => {
    setXiaozhi((current) => ({ ...current, pending: true, tone: "muted", message: "正在保存小智桌面任务权限…" }));
    try {
      const status = await invoke("save_xiaozhi_settings", { input: { otaUrl: null, mcpEnabled } });
      applyXiaozhiStatus(status, { pending: false, tone: "success", message: "设置已保存；重新进入小智实时对话后生效。" });
    } catch (error) {
      setXiaozhi((current) => ({ ...current, pending: false, tone: "error", message: `保存小智桌面任务权限失败：${String(error)}` }));
    }
  };

  const testXiaozhiConnection = async () => {
    setXiaozhi((current) => ({ ...current, pending: true, tone: "muted", message: "正在认证小智 MQTT 连接…" }));
    try {
      const result = await invoke("test_xiaozhi_connection");
      setXiaozhi((current) => ({ ...current, pending: false, tone: "success", message: `MQTT 认证成功 · ${result?.elapsedMs ?? 0}ms` }));
    } catch (error) {
      setXiaozhi((current) => ({ ...current, pending: false, tone: "error", message: `MQTT 连接失败：${String(error)}` }));
    }
  };

  const saveLlm = async () => {
    setVoiceChat((current) => ({ ...current, llmPending: true, llmTone: "muted", llmMessage: "正在保存对话大模型配置…" }));
    try {
      const custom = voiceChat.llmProvider === "custom";
      const status = await invoke("save_voice_chat_settings", {
        input: {
          realtimeEngine: "llm_pipeline",
          llmProvider: voiceChat.llmProvider,
          llmWebSearch: voiceChat.llmWebSearch,
          llmBaseUrl: custom ? voiceChat.llmBaseUrl : null,
          llmModel: voiceChat.llmModel.trim(),
          llmApiKey: voiceChat.llmApiKey.trim() || null,
        },
      });
      emitApiConfigurationUpdated({ providerId: "voice-chat-llm", configured: status?.llmConfigured === true });
      setSavedLlm(null);
      if (status?.llmProvider === "omlx") {
        try {
          const result = await invoke("probe_voice_chat_llm");
          applyVoiceChatStatus(status, {
            llmPending: false,
            llmApiKey: "",
            llmTone: "success",
            llmMessage: `本地 OMLX 已就绪 · ${result.models?.length || 0} 个模型（${result.ms}ms）`,
          });
        } catch (error) {
          applyVoiceChatStatus(status, {
            llmPending: false,
            llmApiKey: "",
            llmTone: "warning",
            llmMessage: `配置已保存，但本地 OMLX 未就绪：${String(error)}`,
          });
        }
        return;
      }
      applyVoiceChatStatus(status, {
        llmPending: false,
        llmApiKey: "",
        llmTone: status?.llmConfigured ? "success" : "warning",
        llmMessage: status?.llmConfigured ? "配置已保存，开始聊天时将使用所选模型。" : "已保存，但还没有 API Key",
      });
    } catch (error) {
      setVoiceChat((current) => ({ ...current, llmPending: false, llmTone: "error", llmMessage: `保存失败：${error}` }));
    }
  };

  useEffect(() => {
    let cancelled = false;
    invoke("load_voice_chat_settings")
      .then((status) => {
        if (!cancelled) applyVoiceChatStatus(status, {
          llmTone: status?.llmConfigured ? "success" : "muted",
          llmMessage: status?.llmConfigured ? "对话大模型已配置" : "选一家大模型，填入 API Key 即可",
        });
        if (status?.llmProvider === "omlx") {
          invoke("probe_voice_chat_llm")
            .then((result) => { if (!cancelled) setVoiceChat((current) => ({ ...current, llmTone: "success", llmMessage: `本地 OMLX 已就绪 · ${result.models?.length || 0} 个模型` })); })
            .catch((error) => { if (!cancelled) setVoiceChat((current) => ({ ...current, llmTone: "warning", llmMessage: `本地 OMLX 未就绪：${String(error)}` })); });
        }
      })
      .catch((error) => {
        if (!cancelled) setVoiceChat((current) => ({ ...current, loading: false,
          llmTone: "error", llmMessage: `读取对话模型配置失败：${error}` }));
      });
    invoke("load_xiaozhi_settings")
      .then((status) => {
        if (!cancelled) applyXiaozhiStatus(status, {
          tone: status?.configured ? "success" : "muted",
          message: status?.configured ? "小智 MQTT 配置已就绪" : "获取绑定状态后，按页面提示完成绑定。",
        });
      })
      .catch((error) => {
        if (!cancelled) setXiaozhi((current) => ({ ...current, loading: false, tone: "error", message: `读取小智配置失败：${String(error)}` }));
      });
    // Independent services: a failed LLM read must not suppress the ASR read.
    invoke("load_device_asr_settings")
      .then((status) => {
        if (cancelled) return;
        setAsrState({
          loading: false,
          pending: false,
          configured: status?.configured === true,
          tone: status?.configured ? "success" : "muted",
          message: status?.message || "尚未配置火山引擎语音识别 API Key",
        });
      })
      .catch((error) => {
        if (cancelled) return;
        setAsrState({
          loading: false,
          pending: false,
          configured: false,
          tone: "error",
          message: `读取语音识别配置失败：${error}`,
        });
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const configuredProviderCount = useMemo(
    () => VIDEO_PROVIDERS.filter((provider) =>
      providerCredentialsConfigured(provider.id, savedVideoConfigs[provider.id]),
    ).length,
    [savedVideoConfigs],
  );
  const configuredCount = configuredProviderCount + (asrState.configured ? 1 : 0)
    + (voiceChat.llmConfigured ? 1 : 0);

  const updateProvider = (providerId, patch) => {
    setVideoConfigs((current) => ({
      ...current,
      [providerId]: {
        ...current[providerId],
        ...patch,
      },
    }));
    setProviderResults((current) => ({ ...current, [providerId]: null }));
  };

  const saveProvider = async (providerId) => {
    if (providerPending[providerId]) return;
    const config = videoConfigs[providerId];
    if (!providerCredentialsConfigured(providerId, config)) {
      setProviderResults((current) => ({
        ...current,
        [providerId]: {
          tone: "warning",
          message: providerId === "kling"
            ? "请填写 Access Key 和 Secret Key。"
            : "请填写 API Key。",
        },
      }));
      return;
    }
    if (providerId === "custom") {
      try {
        new URL(config.baseUrl);
      } catch {
        setProviderResults((current) => ({
          ...current,
          [providerId]: { tone: "error", message: "请填写完整且有效的 Base URL。" },
        }));
        return;
      }
    }
    setProviderPending(current => ({ ...current, [providerId]: true }));
    try {
      await saveProviderConfig(providerId, persistedProviderConfig(config));
    } catch (error) {
      setProviderResults((current) => ({ ...current,
        [providerId]: { tone: "error", message: error.message || "配置未保存，请重试。" },
      }));
      return;
    } finally {
      setProviderPending(current => ({ ...current, [providerId]: false }));
    }
    setSavedVideoConfigs(current => ({ ...current, [providerId]: config }));
    setProviderResults((current) => ({
      ...current,
      [providerId]: { tone: "success", message: "已保存到当前客户端。" },
    }));
    emitApiConfigurationUpdated({ providerId });
  };

  const saveAndTestAsr = async () => {
    /* 本地 ASR 不需要火山 Key：保存偏好 + 预热所选引擎 + 报状态。 */
    if (asrProvider === "local") {
      setAsrState((current) => ({ ...current, pending: true, tone: "muted", message: "正在保存并测试本地 ASR…" }));
      const started = performance.now();
      const engine = localAsrEngine || "sensevoice";
      try {
        /* 之前只 fetch /health 从不保存：引擎选择只留在 localStorage，
         * 真实识别链路（读 ~/.hachimo-local-asr.json）永远拿到旧引擎。 */
        await invoke("local_asr_save_prefs", {
          provider: "local",
          endpoint: localAsrEndpoint,
          engine,
        });
        const health = await invoke("local_asr_health", { endpoint: localAsrEndpoint, engine });
        setAsrState((current) => ({
          ...current,
          configured: true,
          pending: false,
          tone: "success",
          message: `已保存并就绪：${health.engine || engine}${
            health.loaded ? "（已加载）" : "（未加载）"} · ${Math.round(performance.now() - started)}ms`,
        }));
      } catch (error) {
        setAsrState((current) => ({
          ...current,
          pending: false,
          tone: "error",
          message: `本地 ASR 保存/连接失败：${String(error)}`,
        }));
      }
      return;
    }
    if (!asrState.configured && !asrApiKey.trim()) {
      setAsrState((current) => ({
        ...current,
        tone: "warning",
        message: "请填写火山引擎语音识别 API Key。",
      }));
      return;
    }
    let saved = null;
    setAsrState((current) => ({
      ...current,
      pending: true,
      tone: "muted",
      message: "正在保存并测试语音识别服务…",
    }));
    try {
      saved = await invoke("save_device_asr_settings", {
        input: {
          apiKey: asrApiKey.trim() || null,
        },
      });
      emitApiConfigurationUpdated({
        providerId: "volcengine-asr",
        configured: saved?.configured === true,
      });
      setAsrApiKey("");
      const probe = await invoke("test_device_asr_settings");
      setAsrApiKey("");
      setAsrState({
        loading: false,
        pending: false,
        configured: saved?.configured === true,
        tone: "success",
        message: `Key 已保存，语音识别测试通过（${probe?.latencyMs ?? 0} ms）。可到「人设与声音」试听，检查语音合成是否可用。`,
      });
    } catch (error) {
      setAsrState((current) => ({
        ...current,
        loading: false,
        pending: false,
        configured: saved?.configured === true || current.configured,
        tone: "error",
        message: `${saved ? "共享 Key 已保存，但识别测试失败" : "保存失败"}：${error}`,
      }));
    }
  };

  return (
    <div className="api-settings-page">
    <PageShell
      title="设置"
      subtitle="API 服务配置"
      actions={(
        <div className="api-settings__page-actions">
          <button
            type="button"
            className="btn-secondary btn-sm"
            onClick={() => setShowSecrets((current) => !current)}
          >
            {showSecrets ? <EyeOff size={14} /> : <Eye size={14} />}
            {showSecrets ? "隐藏输入" : "显示输入"}
          </button>
          {onBack && (
            <button type="button" className="btn-ghost btn-sm" onClick={onBack}>
              <ArrowLeft size={14} />
              返回
            </button>
          )}
        </div>
      )}
    >
      {dirty && <p role="status" className="api-settings__draft">有未保存的修改。切换页面会保留草稿；关闭客户端前请保存。</p>}
      <section className="api-settings__overview" aria-label="API 配置概览">
        <div className="api-settings__overview-icon" aria-hidden="true">
          <ShieldCheck size={22} />
        </div>
        <div className="api-settings__overview-copy">
          <strong>{configReadError ? "已有配置暂时无法读取（不是配置丢失）" : `${configuredCount} / ${VIDEO_PROVIDERS.length + 2} 项凭据已配置`}</strong>
          <span>按需配置即可：给 Agent 语音输入只需语音识别；和宠物聊天还需语音合成与对话大模型；生成形象需配置形象生成服务。</span>
        </div>
        <div className="api-settings__storage-note">
          <KeyRound size={14} aria-hidden="true" />
          API Key 请勿分享给他人
        </div>
      </section>

      <Card
        title="语音识别与合成"
        subtitle={(
          <>
            让设备听懂你说的话，并让宠物开口回应。识别与合成共用一个 API Key。
            <a
              className="api-settings__credential-link"
              href="https://docs.volcengine.com/docs/DoubaoVoice/APIKeyUsage?lang=zh"
              target="_blank"
              rel="noreferrer"
            >
              Key 获取方式
              <ExternalLink size={11} aria-hidden="true" />
            </a>
          </>
        )}
        actions={(
          <span className={`api-settings__status${asrState.configured ? " is-success" : ""}`}>
            {asrState.loading ? <Loader2 size={13} className="spin" /> : <CheckCircle2 size={13} />}
            {asrState.loading ? "读取中" : asrState.configured ? "已配置" : "未配置"}
          </span>
        )}
      >
        <div className="usage-help">
          <p><strong>语音输入：</strong>{usageHelp.voice} {usageHelp.confirm}</p>
          <p><strong>实时对话：</strong>{usageHelp.chat} 还需配置下方「对话大模型」。</p>
          {usageHelp.pending && <p className="usage-help__notice">按键配置有修改，请先同步到设备。</p>}
          <details><summary>使用前需要什么</summary><p>请保持设备连接电脑，并让 Pet Manager 持续运行。默认使用豆包 ASR 2.0 和 TTS 2.0，请为该 Key 开通对应服务。</p></details>
        </div>
        <div className="api-settings__form api-settings__form--speech">
          <div className="ui-field" role="radiogroup" aria-label="语音合成来源">
            <span className="ui-field__label">语音合成来源</span>
            <label style={{ display: "flex", gap: 8, alignItems: "center" }}>
              <input
                type="radio"
                name="tts-provider"
                checked={ttsProvider === "volcengine"}
                onChange={() => chooseTtsProvider("volcengine")}
              />
              <span>火山引擎（与识别共用 Key）</span>
            </label>
            <label style={{ display: "flex", gap: 8, alignItems: "center" }}>
              <input
                type="radio"
                name="tts-provider"
                checked={ttsProvider === "local"}
                onChange={() => chooseTtsProvider("local")}
              />
              <span>本地 macOS 语音（免 Key，离线）</span>
            </label>
            <label style={{ display: "flex", gap: 8, alignItems: "center" }}>
              <input type="radio" name="tts-provider" checked={ttsProvider === "kokoro"} onChange={() => chooseTtsProvider("kokoro")} />
              <span>Kokoro 中文轻量模型（本地原生，免 Key）</span>
            </label>
          </div>
          {ttsProvider === "kokoro" && (
            <div className="usage-help">
              <p>首次下载会安装原生运行时和模型；不需要 Docker。模型加载较慢时，请保持本页打开等待完成。</p>
              <button type="button" className="ui-button" disabled={kokoroState.pending} onClick={async () => {
                setKokoroState({ pending: true, message: "正在下载并安装本地模型，首次可能需要数分钟…" });
                try { const result = await invoke("kokoro_tts_install"); setKokoroState({ pending: false, message: result.message || "Kokoro 模型已安装" }); }
                catch (error) { setKokoroState({ pending: false, message: "安装失败：" + String(error) }); }
              }}>下载并安装模型</button>
              <button type="button" className="ui-button" disabled={kokoroState.pending} onClick={async () => {
                setKokoroState({ pending: true, message: "正在启动 Kokoro 原生模型…" });
                try { const result = await invoke("kokoro_tts_start"); setKokoroState({ pending: false, message: result.message || "Kokoro 已就绪" }); }
                catch (error) { setKokoroState({ pending: false, message: `启动失败：${String(error)}` }); }
              }}>{kokoroState.pending ? "启动中…" : "启动 Kokoro"}</button>
              <button type="button" className="btn-secondary" disabled={kokoroState.pending} onClick={async () => {
                try { const result = await invoke("kokoro_tts_stop"); setKokoroState({ pending: false, message: result.message || "Kokoro 已停止" }); }
                catch (error) { setKokoroState({ pending: false, message: `停止失败：${String(error)}` }); }
              }}>停止</button>
              <label className="ui-field" htmlFor="api-settings-kokoro-voice">
                <span className="ui-field__label">Kokoro 中文音色</span>
                <select
                  id="api-settings-kokoro-voice"
                  className="ui-control"
                  value={kokoroVoice}
                  onChange={(event) => {
                    const next = event.target.value;
                    setKokoroVoice(next);
                    saveTtsPreferences("kokoro", ttsVoice, next).catch((error) => {
                      setKokoroState({ pending: false, message: "保存音色失败：" + String(error) });
                    });
                  }}
                >
                  <option value="zf_001">中文女声 01</option>
                  <option value="zf_002">中文女声 02</option>
                  <option value="zf_003">中文女声 03</option>
                  <option value="zm_009">中文男声 01</option>
                  <option value="zm_010">中文男声 02</option>
                </select>
              </label>
              <button
                type="button"
                className="ui-button"
                disabled={kokoroState.pending}
                onClick={async () => {
                  setKokoroState({ pending: true, message: "正在合成并试听…" });
                  try {
                    const result = await invoke("kokoro_tts_preview", {
                      text: "你好，我是 Kokoro 本地语音合成测试。",
                      voice: kokoroVoice,
                    });
                    await playTtsPreviewWav(result.wavBase64);
                    setKokoroState({ pending: false, message: "Kokoro 试听播放完成" });
                  } catch (error) {
                    setKokoroState({ pending: false, message: "试听失败：" + String(error) });
                  }
                }}
              >
                试听 Kokoro
              </button>
              {kokoroState.message && <p className="usage-help__notice">{kokoroState.message}</p>}
            </div>
          )}
          {ttsProvider === "local" && (
            <>
              <label className="ui-field" htmlFor="api-settings-tts-voice">
                <span className="ui-field__label">系统音色</span>
                <select
                  id="api-settings-tts-voice"
                  className="ui-control"
                  value={ttsVoice}
                  onChange={(event) => {
                    const next = event.target.value;
                    setTtsVoice(next);
                    saveTtsPreferences("local", next).catch((error) => {
                      setTtsProbe({ pending: false, message: "保存系统音色失败：" + String(error) });
                    });
                  }}
                >
                  {(ttsVoices.length ? usableTtsVoices(ttsVoices, ttsVoice) : ["Tingting"]).map((voice) => (
                    <option key={voice} value={voice}>{voice}</option>
                  ))}
                </select>
              </label>
              {ttsVoicesPending && (
                <p className="muted small">正在检测系统音色可用性（首次约需十几秒，之后有缓存）…</p>
              )}
              {!ttsVoicesPending && ttsVoices.length > 0 && (
                <p className="muted small">
                  已过滤 {ttsVoices.filter((item) => item && item.available === false).length} 个无法发声的音色（未下载语音模型）。
                </p>
              )}
              <button
                type="button"
                className="ui-button"
                disabled={ttsProbe.pending}
                onClick={async () => {
                  setTtsProbe({ pending: true, message: "正在合成…" });
                  try {
                    const result = await invoke("local_tts_preview", {
                      text: "你好，我是本地语音合成测试。",
                      voice: ttsVoice,
                    });
                    /* 真的把声音播出来；播完才解锁按钮，期间再点无效。 */
                    setTtsProbe({
                      pending: true,
                      message: `合成成功：${result.voice} · ${result.bytes} 字节 · ${result.ms}ms · 正在播放…`,
                    });
                    try {
                      await playTtsPreviewWav(result.wavBase64);
                      setTtsProbe({ pending: false, message: `已播放完成（${result.voice} · ${result.ms}ms 合成）` });
                    } catch (playError) {
                      setTtsProbe({ pending: false, message: `合成成功但播放失败：${String(playError)}` });
                    }
                  } catch (error) {
                    setTtsProbe({ pending: false, message: `合成失败：${String(error)}` });
                  }
                }}
              >
                试听本地语音
              </button>
              {ttsProbe.message && <p className="usage-help__notice">{ttsProbe.message}</p>}
          <PromptSounds />
            </>
          )}

          <div className="ui-field" role="radiogroup" aria-label="语音识别来源">
            <span className="ui-field__label">语音识别来源</span>
            <label style={{ display: "flex", gap: 8, alignItems: "center" }}>
              <input
                type="radio"
                name="asr-provider"
                value="volcengine"
                checked={asrProvider === "volcengine"}
                onChange={() => {
                  setAsrProvider("volcengine");
                  invoke("local_asr_save_prefs", { provider: "volcengine", endpoint: localAsrEndpoint, engine: localAsrEngine }).catch(() => {});
                }}
              />
              <span>火山引擎（云端，需要 Key）</span>
            </label>
            <label style={{ display: "flex", gap: 8, alignItems: "center" }}>
              <input
                type="radio"
                name="asr-provider"
                value="local"
                checked={asrProvider === "local"}
                onChange={() => {
                  setAsrProvider("local");
                  invoke("local_asr_save_prefs", { provider: "local", endpoint: localAsrEndpoint, engine: localAsrEngine }).catch(() => {});
                }}
              />
              <span>本地 ASR（sherpa-onnx，免 Key，离线）</span>
            </label>
          </div>
          {asrProvider === "local" && (
            <>
              <label className="ui-field" htmlFor="api-settings-local-asr-endpoint">
                <span className="ui-field__label">本地服务地址</span>
                <input
                  id="api-settings-local-asr-endpoint"
                  className="ui-control"
                  value={localAsrEndpoint}
                  onChange={(event) => {
                    const endpoint = event.target.value;
                    setLocalAsrEndpoint(endpoint);
                    invoke("local_asr_save_prefs", { provider: asrProvider, endpoint, engine: localAsrEngine }).catch((error) => {
                      setLocalAsrProbe({ pending: false, message: `保存服务地址失败：${String(error)}` });
                    });
                  }}
                  placeholder="http://127.0.0.1:8900"
                />
              </label>
              <label className="ui-field" htmlFor="api-settings-local-asr-engine">
                <span className="ui-field__label">识别引擎</span>
                <select
                  id="api-settings-local-asr-engine"
                  className="ui-control"
                  value={localAsrEngine}
                  onChange={(event) => {
                    const engine = event.target.value;
                    setLocalAsrEngine(engine);
                    /* 引擎必须立即写入后端统一配置；真实识别链路不读取浏览器缓存。 */
                    invoke("local_asr_save_prefs", {
                      provider: "local",
                      endpoint: localAsrEndpoint,
                      engine,
                    }).catch((error) => {
                      setLocalAsrProbe({ pending: false, message: `保存引擎偏好失败：${String(error)}` });
                    });
                  }}
                >
                  <option value="sensevoice">SenseVoice（快，0.2 秒 / 900MB）</option>
                  <option value="qwen3">Qwen3-ASR（准，2 秒 / 5GB）</option>
                </select>
              </label>
              <button
                type="button"
                className="ui-button"
                disabled={localAsrProbe.pending}
                onClick={async () => {
                  setLocalAsrProbe({ pending: true, message: "正在连接本地 ASR…" });
                  try {
                    /* 测试前先把当前选择落盘，保证"测试的就是保存的"。 */
                    await invoke("local_asr_save_prefs", {
                      provider: "local",
                      endpoint: localAsrEndpoint,
                      engine: localAsrEngine || "sensevoice",
                    });
                    const health = await invoke("local_asr_health", {
                      endpoint: localAsrEndpoint,
                      engine: localAsrEngine || "sensevoice",
                    });
                    setLocalAsrProbe({
                      pending: false,
                      message: `本地 ASR 已连通：${health.engine}${health.loaded ? "（已加载）" : "（未加载）"} · ${health.latencyMs}ms`,
                    });
                  } catch (error) {
                    setLocalAsrProbe({ pending: false, message: `连接失败：${String(error)}` });
                  }
                }}
              >
                测试本地连接
              </button>
              {localAsrProbe.message && (
                <p className="usage-help__notice">{localAsrProbe.message}</p>
              )}
            </>
          )}
          {asrProvider === "volcengine" && (
          <label className="ui-field" htmlFor="api-settings-asr-key">
            <span className="ui-field__label">语音 API Key（识别与合成共用）</span>
            <input
              id="api-settings-asr-key"
              className="ui-control api-settings__secret-input"
              type={showSecrets ? "text" : "password"}
              autoComplete="new-password"
              value={asrApiKey}
              onChange={(event) => setAsrApiKey(event.target.value)}
              placeholder={asrState.configured ? "已安全保存；留空可直接复测" : "输入火山引擎豆包语音 API Key"}
              disabled={asrState.pending || asrState.loading}
            />
          </label>
          )}
          <button
            type="button"
            className="btn-primary btn-sm api-settings__save"
            onClick={saveAndTestAsr}
            disabled={asrState.pending || asrState.loading}
          >
            {asrState.pending ? <Loader2 size={14} className="spin" /> : <CheckCircle2 size={14} />}
            保存并测试
          </button>
        </div>
        <div className={`api-settings__result${resultToneClass(asrState.tone)}`} role="status">
          {asrState.tone === "error" || asrState.tone === "warning"
            ? <AlertCircle size={13} />
            : <CheckCircle2 size={13} />}
          {asrState.message}
        </div>
      </Card>

      <Card title="对话大模型" subtitle="决定宠物在「实时对话」中如何理解和回答。选择一家服务并填写 API Key 即可。此配置用于和宠物聊天，不影响 ChatGPT、Claude 等 Agent 的语音输入。">
        <UsageHelp />
        <div className="api-settings__presets" role="radiogroup" aria-label="实时对话引擎">
          <button type="button" role="radio" aria-checked={voiceChat.realtimeEngine !== "xiaozhi"}
            className={"btn-ghost btn-sm api-settings__preset" + (voiceChat.realtimeEngine !== "xiaozhi" ? " is-active" : "")}
            onClick={() => selectRealtimeEngine("llm_pipeline")} disabled={voiceChat.llmPending || voiceChat.loading}>
            模型链路
          </button>
          <button type="button" role="radio" aria-checked={voiceChat.realtimeEngine === "xiaozhi"}
            className={"btn-ghost btn-sm api-settings__preset" + (voiceChat.realtimeEngine === "xiaozhi" ? " is-active" : "")}
            onClick={() => selectRealtimeEngine("xiaozhi")} disabled={voiceChat.llmPending || voiceChat.loading}>
            小智 AI
          </button>
        </div>
        {voiceChat.realtimeEngine === "xiaozhi" ? (
          <div className="api-settings__form api-settings__form--llm">
            <p className="muted small">小智用于实时问答；启用下方选项后，可将明确的电脑任务派发给当前跟随的 Agent，不影响其他实时对话引擎。</p>
            <label className="ui-field" htmlFor="api-settings-xiaozhi-ota-url">
              <span className="ui-field__label">小智 OTA 地址</span>
              <input id="api-settings-xiaozhi-ota-url" className="ui-control" value={xiaozhi.otaUrl}
                onChange={(event) => setXiaozhi((current) => ({ ...current, otaUrl: event.target.value }))}
                disabled={xiaozhi.pending || xiaozhi.loading} />
            </label>
            <label className="ui-field" htmlFor="api-settings-xiaozhi-mcp-enabled">
              <span className="ui-field__label">允许小智派发桌面任务</span>
              <input id="api-settings-xiaozhi-mcp-enabled" type="checkbox" checked={xiaozhi.mcpEnabled}
                onChange={(event) => toggleXiaozhiMcp(event.target.checked)}
                disabled={xiaozhi.pending || xiaozhi.loading} />
              <span className="muted small">派发目标为当前跟随的 Codex 或 WorkBuddy；设置后重新进入实时对话生效。</span>
            </label>
            <div className="api-settings__preset-summary muted small">
              {xiaozhi.activationState === "ready" ? "已绑定" : xiaozhi.activationState === "pending" ? "等待绑定" : "尚未获取绑定状态"}
              {xiaozhi.deviceIdMasked ? ` · 设备 ${xiaozhi.deviceIdMasked}` : ""}
            </div>
            {xiaozhi.activationCode ? <div className="api-settings__preset-summary">
              绑定码：<strong>{xiaozhi.activationCode}</strong>
              <a className="api-settings__credential-link" href="https://xiaozhi.me" target="_blank" rel="noreferrer">
                打开小智绑定页
                <ExternalLink size={11} aria-hidden="true" />
              </a>
            </div> : null}
            <div className="api-settings__actions">
              <button type="button" className="btn-primary btn-sm api-settings__save" onClick={activateXiaozhi} disabled={xiaozhi.pending || xiaozhi.loading}>
                {xiaozhi.pending ? <Loader2 size={14} className="spin" /> : <CheckCircle2 size={14} />}
                获取绑定状态
              </button>
              <button type="button" className="btn-ghost btn-sm" onClick={testXiaozhiConnection} disabled={xiaozhi.pending || xiaozhi.loading || !xiaozhi.configured}>
                测试 MQTT 连接
              </button>
            </div>
            <div className={`api-settings__result${resultToneClass(xiaozhi.tone)}`} role="status">
              {xiaozhi.tone === "error" || xiaozhi.tone === "warning" ? <AlertCircle size={13} /> : <CheckCircle2 size={13} />}
              {xiaozhi.message}
            </div>
          </div>
        ) : <>
        <div className="api-settings__presets" role="radiogroup" aria-label="对话大模型">
          {[...LLM_PRESETS, { id: "custom", label: "自定义" }].map((preset) => (
            <button
              key={preset.id}
              type="button"
              role="radio"
              aria-checked={voiceChat.llmProvider === preset.id}
              className={"btn-ghost btn-sm api-settings__preset" + (voiceChat.llmProvider === preset.id ? " is-active" : "")}
              onClick={() => setVoiceChat((current) => selectLlmProvider(current, preset.id))}
              disabled={voiceChat.llmPending || voiceChat.loading}
            >
              {preset.label}
            </button>
          ))}
        </div>
        <div className="api-settings__form api-settings__form--llm">
          {voiceChat.llmProvider === "custom" ? (
            <>
              <label className="ui-field" htmlFor="api-settings-llm-url">
                <span className="ui-field__label">接口地址（OpenAI 兼容）</span>
                <input
                  id="api-settings-llm-url"
                  className="ui-control"
                  value={voiceChat.llmBaseUrl}
                  onChange={(event) => setVoiceChat((current) => changeLlmEndpoint(current, event.target.value))}
                  placeholder="https://your-endpoint/v1"
                  disabled={voiceChat.llmPending || voiceChat.loading}
                />
              </label>
            </>
          ) : (
            <div className="api-settings__preset-summary muted small">
              {describeLlmPreset(voiceChat.llmProvider, voiceChat.llmModel)}
            </div>
          )}
          <label className="ui-field" htmlFor="api-settings-llm-model">
            <span className="ui-field__label">模型名称 / 接入点 ID</span>
            <TextModelSelect value={voiceChat.llmModel}
              onChange={llmModel => setVoiceChat(current => ({ ...current, llmModel }))}
              provider={voiceChat.llmProvider} baseUrl={voiceChat.llmBaseUrl}
              apiKey={voiceChat.llmApiKey} configured={!voiceChat.loading && voiceChat.llmConfigured}
              disabled={voiceChat.llmPending || voiceChat.loading} />
            <span id="api-settings-llm-model-help" className="muted small">可填写账号已开通的模型名称或接入点 ID；留空使用默认模型。</span>
            <button
              type="button"
              className="ui-button"
              disabled={voiceChat.llmPending || voiceChat.loading}
              onClick={async () => {
                setVoiceChat((current) => ({ ...current, llmTone: "muted", llmMessage: "正在获取模型列表…" }));
                try {
                  /* 走 Rust 侧请求：本地 OMLX / 云端都绕开 WebView 的 CORS */
                  const result = await invoke("list_openai_models", {
                    baseUrl: voiceChat.llmBaseUrl,
                    /* OMLX 的本地令牌默认是 1234；用户填了就用用户的 */
                    apiKey: voiceChat.llmApiKey || (voiceChat.llmProvider === "omlx" ? "1234" : null),
                  });
                  const first = result.models[0];
                  setVoiceChat((current) => ({
                    ...current,
                    llmModel: current.llmModel || first,
                    llmTone: "success",
                    llmMessage: `获取到 ${result.models.length} 个模型（${result.ms}ms）：${result.models.slice(0, 3).join("、")}${result.models.length > 3 ? " …" : ""}`,
                  }));
                } catch (error) {
                  setVoiceChat((current) => ({ ...current, llmTone: "error", llmMessage: `获取模型列表失败：${String(error)}` }));
                }
              }}
            >
              获取模型列表
            </button>
          </label>
          <label className="ui-field" htmlFor="api-settings-llm-key">
            <span className="ui-field__label">API Key</span>
            <input
              id="api-settings-llm-key"
              className="ui-control api-settings__secret-input"
              type={showSecrets ? "text" : "password"}
              autoComplete="new-password"
              value={voiceChat.llmApiKey}
              onChange={(event) => setVoiceChat((current) => ({ ...current, llmApiKey: event.target.value }))}
              placeholder={voiceChat.llmConfigured ? `已保存 ${voiceChat.llmApiKeyMasked}；留空保持不变` : "粘贴这家模型的 API Key"}
              disabled={voiceChat.llmPending || voiceChat.loading}
            />
          </label>
          <button
            type="button"
            className="btn-primary btn-sm api-settings__save"
            onClick={saveLlm}
            disabled={voiceChat.llmPending || voiceChat.loading}
          >
            {voiceChat.llmPending ? <Loader2 size={14} className="spin" /> : <CheckCircle2 size={14} />}
            保存
          </button>
        </div>
        <div className={`api-settings__result${resultToneClass(voiceChat.llmTone)}`} role="status">
          {voiceChat.llmTone === "error" || voiceChat.llmTone === "warning"
            ? <AlertCircle size={13} />
            : <CheckCircle2 size={13} />}
          {voiceChat.llmMessage}
        </div>
        <label className="ui-field api-settings__search-option">
          <span><input type="checkbox" checked={voiceChat.llmWebSearch}
            disabled={voiceChat.llmPending || voiceChat.loading}
            onChange={event => setVoiceChat(current => ({ ...current, llmWebSearch: event.target.checked }))} /> 按需联网检索</span>
          <span className="muted small">询问最新消息时查询，闲聊和家居控制不查询。使用当前服务商的 Key；模型需支持原生搜索，账号可能需开通权限并产生额外费用。查询后由宠物直接回答。修改后点击保存，下次开始聊天生效。</span>
        </label>
        <LlmNetworkSettings />
        </>}
      </Card>

      <Card title="形象生成" subtitle="用于新形象生成与单状态视频替换">
        <GenerationDiagnostics />
        <div className="api-settings__provider-grid">
          {VIDEO_PROVIDERS.map((provider) => {
            const config = videoConfigs[provider.id];
            const configured = providerCredentialsConfigured(provider.id, savedVideoConfigs[provider.id]);
            const result = providerResults[provider.id];
            const isKling = provider.id === "kling";
            const isCustom = provider.id === "custom";
            return (
              <section className="api-settings__provider" key={provider.id}>
                <header className="api-settings__provider-head">
                  <span className="api-settings__service-mark" aria-hidden="true">
                    <Sparkles size={18} />
                  </span>
                  <span className="api-settings__provider-title">
                    <strong>{provider.label}</strong>
                    <small>
                      {provider.sub}
                      {provider.id === "volcengine" && (
                        <a
                          className="api-settings__credential-link"
                          href="https://ark.volcengine.com/model/detail?name=doubao-seedance-2-0-mini"
                          target="_blank"
                          rel="noreferrer"
                        >
                          Key 获取方式
                          <ExternalLink size={11} aria-hidden="true" />
                        </a>
                      )}
                    </small>
                  </span>
                  <span className={`api-settings__status${configured ? " is-success" : ""}`}>
                    {configReadError ? "读取失败" : configured ? "已配置" : "未配置"}
                  </span>
                </header>

                <fieldset className="api-settings__provider-fields" disabled={Boolean(configReadError)} style={{border:0,padding:0,margin:0,minWidth:0}}>
                  {isKling ? (
                    <>
                      <label className="ui-field" htmlFor="api-settings-kling-access">
                        <span className="ui-field__label">Access Key</span>
                        <input
                          id="api-settings-kling-access"
                          className="ui-control api-settings__secret-input"
                          type={showSecrets ? "text" : "password"}
                          autoComplete="new-password"
                          value={config.accessKey}
                          onChange={(event) => updateProvider(provider.id, { accessKey: event.target.value })}
                          placeholder="Kling Access Key"
                        />
                      </label>
                      <label className="ui-field" htmlFor="api-settings-kling-secret">
                        <span className="ui-field__label">Secret Key</span>
                        <input
                          id="api-settings-kling-secret"
                          className="ui-control api-settings__secret-input"
                          type={showSecrets ? "text" : "password"}
                          autoComplete="new-password"
                          value={config.secretKey}
                          onChange={(event) => updateProvider(provider.id, { secretKey: event.target.value })}
                          placeholder="Kling Secret Key"
                        />
                      </label>
                    </>
                  ) : (
                    <label className="ui-field" htmlFor={`api-settings-${provider.id}-key`}>
                      <span className="ui-field__label">API Key</span>
                      <input
                        id={`api-settings-${provider.id}-key`}
                        className="ui-control api-settings__secret-input"
                        type={showSecrets ? "text" : "password"}
                        autoComplete="new-password"
                        value={config.apiKey === "hachimidock-bundled-ark-v1" ? "" : config.apiKey}
                        onChange={(event) => updateProvider(provider.id, { apiKey: event.target.value })}
                        placeholder={config.apiKey === "hachimidock-bundled-ark-v1" ? "内置服务已启用，可填写自己的 Key 替换" : `输入${provider.label} API Key`}
                      />
                    </label>
                  )}
                  {isCustom && (
                    <label className="ui-field" htmlFor="api-settings-custom-url">
                      <span className="ui-field__label">Base URL</span>
                      <input
                        id="api-settings-custom-url"
                        className="ui-control api-settings__secret-input"
                        value={config.baseUrl}
                        onChange={(event) => updateProvider(provider.id, { baseUrl: event.target.value })}
                        placeholder="https://api.example.com"
                      />
                    </label>
                  )}
                  {provider.id === "volcengine" && <div className="ui-field">
                    <span className="ui-field__label">视频生成模型</span>
                    <VideoModelSelect apiKey={config.apiKey} baseUrl={config.baseUrl}
                      model={config.model} onModel={model => updateProvider(provider.id, { model })}
                      autoSelectDefault={false} />
                    <span className="muted small">用于生成形象视频；与下方提示词模型独立。修改后点击保存。</span>
                  </div>}
                  {provider.id === "volcengine" && <div className="ui-field">
                    <span className="ui-field__label">图像理解 / 提示词模型</span>
                    <TextModelSelect vision label="图像理解模型" value={config.thinkingModel}
                      onChange={thinkingModel => updateProvider(provider.id, { thinkingModel })}
                      apiKey={config.apiKey} baseUrl={config.baseUrl} />
                  </div>}
                  {provider.id === "volcengine" && <div className="ui-field">
                    <span className="ui-field__label">图片处理模型</span>
                    <ImageModelSelect value={config.imageModel}
                      onChange={imageModel => updateProvider(provider.id, { imageModel })}
                      apiKey={config.apiKey} baseUrl={config.baseUrl} />
                  </div>}
                </fieldset>

                <footer className="api-settings__provider-foot">
                  <span className={`api-settings__provider-result${resultToneClass(result?.tone)}`}>
                    {result?.message || (configReadError ? "原配置保留，暂不可修改" : configured ? "可用于形象生成" : "等待填写凭据")}
                  </span>
                  <button
                    type="button"
                    className="btn-secondary btn-sm"
                    onClick={() => saveProvider(provider.id)}
                    disabled={providerPending[provider.id] === true || Boolean(configReadError)}
                  >
                    {providerPending[provider.id] ? "保存中…" : "保存"}
                  </button>
                </footer>
              </section>
            );
          })}
        </div>
      </Card>
    </PageShell>
    </div>
  );
}
