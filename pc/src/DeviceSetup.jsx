/**
 * [Input] Tauri USB scan/connect/status commands and persisted device bindings.
 * [Output] USB setup within normal navigation; discovers verified devices and only locks completion after successful dashboard handoff, retaining retry on readback failure.
 * [Pos] component node in pc/src
 * [Sync] If this file changes, update this header and `pc/src/.folder.md`.
 */

import React, { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ensureBridgeRuntime } from "./lib/bridge-runtime.js";
import {
  Cable,
  CheckCircle2,
  Loader2,
  RefreshCw,
  Usb,
  Wifi,
} from "lucide-react";

const AUTO_SCAN_INTERVAL_MS = 3000;
const TCP_HOST_STORAGE_KEY = "pet-manager:tcp-host";
const DEFAULT_TCP_HOST = "192.168.1.97";
const DEFAULT_TCP_PORT = 3333;

function rememberedTcpHost() {
  try {
    const stored = window.localStorage.getItem(TCP_HOST_STORAGE_KEY);
    return String(stored || "").trim() || DEFAULT_TCP_HOST;
  } catch {
    return DEFAULT_TCP_HOST;
  }
}

function rememberTcpHost(host) {
  try {
    window.localStorage.setItem(TCP_HOST_STORAGE_KEY, String(host || "").trim());
  } catch {
    // Storage may be unavailable; the default host still applies.
  }
}

function hasTauriRuntime() {
  return typeof window !== "undefined" && Boolean(window.__TAURI_INTERNALS__);
}

function stableBindings(bindings) {
  return Array.isArray(bindings)
    ? bindings.filter((binding) => String(binding?.boardDeviceId || "").trim())
    : [];
}

function normalizedUsbStatus(status) {
  return {
    connected: Boolean(status?.connected),
    portName: String(status?.portName || "").trim(),
    boardDeviceId: String(status?.boardDeviceId || "").trim(),
    runtime: String(status?.runtime || "").trim(),
  };
}

export default function DeviceSetup({ onComplete } = {}) {
  const [phase, setPhase] = useState("scanning");
  const [message, setMessage] = useState("正在自动查找已连接的桌宠…");
  const [device, setDevice] = useState(null);
  const [tcpHost, setTcpHost] = useState(rememberedTcpHost);
  const [tcpPending, setTcpPending] = useState(false);
  const scanInFlightRef = useRef(null);
  const completedRef = useRef(false);
  const mountedRef = useRef(true);

  const finish = useCallback(async () => {
    if (completedRef.current) return;
    await onComplete?.();
    completedRef.current = true;
  }, [onComplete]);

  const persistConnectedDevice = useCallback(async (rawStatus) => {
    const status = normalizedUsbStatus(rawStatus);
    if (!status.connected || !status.boardDeviceId) {
      throw new Error("USB 已连接，但设备没有返回有效身份");
    }

    const desktopDeviceId = await invoke("get_or_create_desktop_device_id");
    const transportLabel = String(status.portName || "").startsWith("tcp://") ? "TCP" : "USB";
    await invoke("save_device_binding", {
      binding: {
        boardDeviceId: status.boardDeviceId,
        desktopDeviceId,
        wifiSsid: `${transportLabel}(${status.portName || "direct"})`,
        boundAt: Date.now(),
      },
    });
    ensureBridgeRuntime().catch(() => {});

    if (!mountedRef.current) return;
    setDevice(status);
    setPhase("connected");
    setMessage("设备已连接，正在进入管理页面…");
    await finish();
  }, [finish]);

  // Route A fallback: the board serves the same verified protocol over TCP
  // (:3333) whenever it is on Wi-Fi, so a device that no longer shows up as a
  // USB serial port can still complete setup over the network.
  const connectTcp = useCallback(async ({ quiet = false } = {}) => {
    if (completedRef.current) return;
    const host = String(tcpHost || "").trim();
    if (!host) throw new Error("请输入设备的 IP 地址");
    if (!quiet && mountedRef.current) {
      setPhase("scanning");
      setMessage(`正在通过 WiFi 连接 ${host}:${DEFAULT_TCP_PORT}…`);
    }
    rememberTcpHost(host);
    const status = normalizedUsbStatus(
      await invoke("usb_connect_tcp", { host, port: DEFAULT_TCP_PORT }),
    );
    if (!status.connected || !status.boardDeviceId) {
      throw new Error("TCP 已连接，但设备没有返回有效身份");
    }
    await persistConnectedDevice(status);
  }, [tcpHost, persistConnectedDevice]);

  const scan = useCallback(async ({ quiet = false } = {}) => {
    if (completedRef.current) return;
    if (scanInFlightRef.current) return scanInFlightRef.current;

    const request = (async () => {
      if (!quiet && mountedRef.current) {
        setPhase("scanning");
        setMessage("正在扫描 USB 设备…");
      }

      try {
        const bindings = stableBindings(await invoke("load_device_bindings"));
        if (bindings.length > 0) {
          await finish();
          return;
        }

        let status = normalizedUsbStatus(await invoke("usb_get_status"));
        if (!status.connected || !status.boardDeviceId) {
          const candidates = await invoke("usb_scan_devices");
          for (const candidate of Array.isArray(candidates) ? candidates : []) {
            const portName = String(candidate?.portName || "").trim();
            if (!portName) continue;
            try {
              status = normalizedUsbStatus(await invoke("usb_connect", { portName }));
              if (status.connected && status.boardDeviceId) break;
            } catch {
              // A serial port is not necessarily a Pet Manager device. Keep probing.
            }
          }
        }

        if (status.connected && status.boardDeviceId) {
          await persistConnectedDevice(status);
          return;
        }

        if (mountedRef.current) {
          setDevice(null);
          setPhase("waiting");
          setMessage("暂未检测到桌宠，请确认设备已开机并连接数据线。");
        }
      } catch (error) {
        if (mountedRef.current) {
          setDevice(null);
          setPhase("error");
          setMessage(error?.message || String(error));
        }
      }
    })();

    scanInFlightRef.current = request;
    try {
      await request;
    } finally {
      if (scanInFlightRef.current === request) {
        scanInFlightRef.current = null;
      }
    }
  }, [finish, persistConnectedDevice]);

  useEffect(() => {
    mountedRef.current = true;
    scan();
    const intervalId = window.setInterval(
      () => scan({ quiet: true }),
      AUTO_SCAN_INTERVAL_MS,
    );
    return () => {
      mountedRef.current = false;
      window.clearInterval(intervalId);
    };
  }, [scan]);

  if (!hasTauriRuntime()) {
    return (
      <div className="setup--wizard usb-setup">
        <section className="usb-setup-card">
          <span className="usb-setup-card__icon">
            <Usb size={28} />
          </span>
          <h2>请在 Pet Manager 客户端中连接设备</h2>
          <p>浏览器预览不支持 USB 设备发现。</p>
        </section>
      </div>
    );
  }

  const busy = phase === "scanning" || phase === "connected";

  return (
    <div className="setup--wizard usb-setup">
      <section className="usb-setup-card" aria-live="polite">
        <div className={`usb-setup-card__icon usb-setup-card__icon--${phase}`}>
          {phase === "connected"
            ? <CheckCircle2 size={30} />
            : busy
              ? <Loader2 size={30} className="spin" />
              : <Usb size={30} />}
        </div>

        <div className="usb-setup-card__copy">
          <span className="usb-setup-card__eyebrow">USB 自动连接</span>
          <h2>{phase === "connected" ? "已发现桌宠" : "连接你的桌宠"}</h2>
          <p>{message}</p>
        </div>

        {device ? (
          <dl className="usb-setup-device">
            <div>
              <dt>设备</dt>
              <dd>{device.boardDeviceId}</dd>
            </div>
            <div>
              <dt>接口</dt>
              <dd>{device.portName || "USB"}</dd>
            </div>
          </dl>
        ) : (
          <div className="usb-setup-checklist">
            <span><Cable size={18} /> 使用可传输数据的 USB 线连接设备</span>
            <span><CheckCircle2 size={18} /> 保持设备开机，Pet Manager 会自动识别</span>
          </div>
        )}

        {!device && !busy ? (
          <div className="usb-setup-tcp">
            <span className="usb-setup-tcp__label"><Wifi size={16} /> 设备在同一 WiFi 时可免 USB 直连</span>
            <div className="usb-setup-tcp__row">
              <input
                type="text"
                value={tcpHost}
                onChange={(event) => setTcpHost(event.target.value)}
                placeholder="设备 IP，例如 192.168.1.97"
                aria-label="设备 TCP 地址"
              />
              <button
                className="btn-primary usb-setup-tcp__action"
                type="button"
                disabled={tcpPending || !String(tcpHost || "").trim()}
                onClick={() => {
                  setTcpPending(true);
                  connectTcp()
                    .catch((error) => {
                      if (mountedRef.current) {
                        setPhase("waiting");
                        setMessage(error?.message || String(error));
                      }
                    })
                    .finally(() => {
                      if (mountedRef.current) setTcpPending(false);
                    });
                }}
              >
                {tcpPending ? <Loader2 size={16} className="spin" /> : <Wifi size={16} />}
                WiFi 连接
              </button>
            </div>
          </div>
        ) : null}

        <button
          className="btn-primary usb-setup-card__action"
          type="button"
          disabled={busy}
          onClick={() => scan()}
        >
          {busy ? <Loader2 size={16} className="spin" /> : <RefreshCw size={16} />}
          {busy ? "正在检测" : "重新扫描"}
        </button>

        {phase === "error" ? (
          <p className="usb-setup-card__error">检测失败：{message}</p>
        ) : null}
      </section>
    </div>
  );
}
