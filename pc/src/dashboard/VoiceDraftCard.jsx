/**
 * [Input] Tauri voice-draft-* events (emitted by device_voice_bridge.rs) and the
 *         three voice_draft_* commands (status/submit/cancel).
 * [Output] A visible "recognized text, awaiting confirmation" card with explicit
 *          Send / Cancel buttons. The same draft can also be confirmed from the
 *          device with joystick right / confirm key, or cancelled with joystick
 *          left / back key — see try_draft_gesture in device_voice_bridge.rs.
 * [Pos] dashboard component in pc/src/dashboard
 */
import React, { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import Card from "../shell/Card.jsx";
import Button from "../shell/Button.jsx";
import { useToast } from "../shell/ToastStack.jsx";

export default function VoiceDraftCard() {
  const [draft, setDraft] = useState(null);
  const [busy, setBusy] = useState(false);
  const { push } = useToast();

  useEffect(() => {
    let disposed = false;
    const unlisteners = [];
    invoke("voice_draft_status")
      .then((status) => {
        if (!disposed && status?.pending) setDraft(status);
      })
      .catch(() => {});
    const attach = (event, handler) => {
      listen(event, (payload) => {
        if (!disposed) handler(payload?.payload || {});
      })
        .then((stop) => {
          if (disposed) stop();
          else unlisteners.push(stop);
        })
        .catch(() => {});
    };
    attach("voice-draft-ready", (next) => setDraft(next));
    attach("voice-draft-cleared", () => setDraft(null));
    attach("voice-draft-result", (result) => {
      setBusy(false);
      push({
        tone: result?.ok ? "success" : "error",
        title: result?.cancelled ? "语音草稿已取消" : result?.ok ? "已请求提交" : "未能确认发送",
        message: String(result?.message || "").slice(0, 160),
        ttl: 5_000,
      });
    });
    return () => {
      disposed = true;
      unlisteners.forEach((stop) => { try { stop(); } catch { /* ignore */ } });
    };
  }, [push]);

  const submit = useCallback(async () => {
    if (busy || !draft) return;
    setBusy(true);
    try {
      await invoke("voice_draft_submit");
      setDraft(null);
    } catch (error) {
      setBusy(false);
      push({
        tone: "error",
        title: "发送失败",
        message: String(error?.message || error).slice(0, 160),
        ttl: 6_000,
      });
    }
  }, [busy, draft, push]);

  const cancel = useCallback(async () => {
    if (busy || !draft) return;
    setBusy(true);
    try {
      await invoke("voice_draft_cancel");
      setDraft(null);
      push({ tone: "info", title: "语音草稿已取消", message: "识别文字未发送给 Agent。", ttl: 3_000 });
    } catch (error) {
      push({
        tone: "error",
        title: "取消失败",
        message: String(error?.message || error).slice(0, 160),
        ttl: 5_000,
      });
    } finally {
      setBusy(false);
    }
  }, [busy, draft, push]);

  if (!draft) return null;

  return (
    <Card
      title="识别结果 · 待确认"
      subtitle={`目标 Agent：${draft.agentId || "未指定"} ｜ 摇杆右/确认键发送，摇杆左/返回键取消`}
      actions={(
        <>
          <Button variant="ghost" size="small" onClick={cancel} disabled={busy}>
            取消
          </Button>
          <Button variant="primary" size="small" onClick={submit} loading={busy} loadingLabel="发送中…">
            发送
          </Button>
        </>
      )}
    >
      <div
        data-testid="voice-draft-text"
        style={{ whiteSpace: "pre-wrap", wordBreak: "break-word", lineHeight: 1.6 }}
      >
        {draft.text}
      </div>
    </Card>
  );
}
