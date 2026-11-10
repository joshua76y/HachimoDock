import React, { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

/* 提示音设置：9 类事件，可上传自定义 WAV 并一键推到设备。
 * 后端已按设备能接受的节奏上传（1KB/40ms/等 ack），这里只负责选择文件。 */
const EVENTS = [
  ["record_start", "开始录音"],
  ["record_stop", "松开结束"],
  ["asr_ok", "识别成功"],
  ["asr_fail", "识别失败"],
  ["send_ok", "草稿已发送"],
  ["task_done", "任务完成"],
  ["task_fail", "任务失败"],
  ["low_battery", "电量偏低"],
  ["wifi_lost", "网络断开"],
];

export default function PromptSounds() {
  const [catalog, setCatalog] = useState({ entries: [], soundsDir: "" });
  const [host, setHost] = useState("192.168.1.97");
  const [status, setStatus] = useState({ pending: false, message: "" });

  const refresh = () =>
    invoke("prompt_sound_catalog")
      .then((data) => setCatalog(data))
      .catch((error) => setStatus({ pending: false, message: `读取提示音目录失败：${error}` }));

  useEffect(() => {
    refresh();
  }, []);

  const pick = async (event, file) => {
    if (!file) return;
    setStatus({ pending: true, message: `正在导入 ${event}…` });
    try {
      const bytes = Array.from(new Uint8Array(await file.arrayBuffer()));
      const entry = await invoke("prompt_sound_save_bytes", { event, bytes });
      await refresh();
      setStatus({ pending: false, message: `${entry.label}：已保存 ${entry.bytes} 字节（待下发）` });
    } catch (error) {
      setStatus({ pending: false, message: `导入失败：${String(error)}` });
    }
  };

  const push = async (event) => {
    setStatus({ pending: true, message: `正在下发 ${event} 到设备…` });
    try {
      const message = await invoke("prompt_sound_upload", { event, host });
      setStatus({ pending: false, message });
    } catch (error) {
      setStatus({ pending: false, message: `下发失败：${String(error)}` });
    }
  };

  const reset = async (event) => {
    try {
      await invoke("prompt_sound_reset", { event }).catch(() => {});
      await refresh();
      setStatus({ pending: false, message: `${event}：已恢复内置音` });
    } catch (error) {
      setStatus({ pending: false, message: `恢复失败：${String(error)}` });
    }
  };

  const sourceOf = (event) => catalog.entries?.find((item) => item.event === event);

  return (
    <div className="api-settings__form">
      <span className="ui-field__label">提示音（设备端播放）</span>
      <label className="ui-field">
        <span className="muted small">设备地址</span>
        <input className="ui-control" value={host} onChange={(e) => setHost(e.target.value)} />
      </label>
      <table className="ui-table">
        <tbody>
          {EVENTS.map(([event, label]) => {
            const entry = sourceOf(event);
            const custom = entry?.source === "custom";
            return (
              <tr key={event}>
                <td>{label}</td>
                <td className="muted small">{custom ? `自定义 ${entry?.bytes ?? 0} B` : "内置音"}</td>
                <td style={{ display: "flex", gap: 8 }}>
                  <label className="ui-button" style={{ cursor: "pointer" }}>
                    选文件
                    <input
                      type="file"
                      accept="audio/wav,audio/x-wav,.wav"
                      style={{ display: "none" }}
                      onChange={(e) => pick(event, e.target.files?.[0])}
                    />
                  </label>
                  <button type="button" className="ui-button" disabled={!custom || status.pending} onClick={() => push(event)}>
                    下发
                  </button>
                  <button type="button" className="ui-button" disabled={!custom} onClick={() => reset(event)}>
                    恢复
                  </button>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
      {catalog.soundsDir && <p className="muted small">本地目录：{catalog.soundsDir}</p>}
      {status.message && <p className="usage-help__notice">{status.message}</p>}
    </div>
  );
}
