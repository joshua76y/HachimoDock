/**
 * [Input] useDeviceContext for binding/usb/switchConnectionRoute; useToast for manual result notices.
 * [Output] Region 1 of the P4 dashboard: single-row card showing board id, verified transport state, a persistent USB-first automatic route and explicit USB/Wi-Fi route switches.
 * [Pos] component node in pc/src/dashboard
 * [Sync] If this file changes, update `pc/src/dashboard/.folder.md`.
 */

import React, { useState } from "react";
import { Monitor, Usb, Wifi } from "lucide-react";
import { useDeviceContext } from "../shell/DeviceContext.jsx";
import { useToast } from "../shell/ToastStack.jsx";

export default function DeviceStatusBar() {
  const { binding, usb, switchConnectionRoute } = useDeviceContext();
  const { push } = useToast();
  const boardDeviceId = String(binding?.boardDeviceId || "").trim();
  const savedTcpHost = (() => {
    try {
      return String(
        binding?.tcpHost || window.localStorage.getItem("pet-manager:tcp-host") || "192.168.1.97",
      ).trim();
    } catch {
      return String(binding?.tcpHost || "192.168.1.97").trim();
    }
  })();
  const [tcpHostDraft, setTcpHostDraft] = useState({
    boardDeviceId: "",
    value: "",
    edited: false,
  });
  const tcpHost = tcpHostDraft.boardDeviceId === boardDeviceId && tcpHostDraft.edited
    ? tcpHostDraft.value
    : savedTcpHost;
  const [switchingRoute, setSwitchingRoute] = useState(false);

  const handleRouteSelect = async (route) => {
    const host = String(tcpHost || "").trim();
    if (route === "tcp" && !host) return;
    setSwitchingRoute(true);
    try {
      const result = await switchConnectionRoute({
        route,
        host: tcpHost,
        port: 3333,
      });
      const status = result?.status;
      if (status?.connected) {
        const isTcp = status.transport === "tcp"
          || String(status.portName || "").startsWith("tcp://");
        const selectedLabel = route === "auto" ? "自动连接" : route === "usb" ? "USB" : "Wi-Fi";
        push({
          tone: "success",
          title: `${selectedLabel}：已连接${isTcp ? " Wi-Fi" : " USB"} ${status.portName || ""}`.trim(),
        });
      } else {
        push({
          tone: "warning",
          title: route === "usb" ? "未检测到 USB 设备" : "暂未建立设备连接",
          message: route === "usb"
            ? "请确认数据线已接入后再选择 USB。"
            : "自动模式会优先使用 USB；未连接时会尝试已保存的 Wi-Fi 地址。",
        });
      }
    } catch (error) {
      push({
        tone: "error",
        title: route === "tcp" ? "Wi-Fi 连接失败" : "连接方式切换失败",
        message: String(error?.message || error),
      });
    } finally {
      setSwitchingRoute(false);
    }
  };

  if (!binding) return null;

  const legacyTransportLabel = String(binding.wifiSsid || "").trim();
  const routePreference = ["auto", "usb", "tcp"].includes(
    String(binding.routePreference || "").trim().toLowerCase(),
  ) ? String(binding.routePreference).trim().toLowerCase() : "auto";
  const usbOnlyBindingMatch = legacyTransportLabel.match(/^USB\((.+)\)$/);
  const savedUsbPortName = usbOnlyBindingMatch?.[1] || "";
  const isTcp = usb.transport === "tcp" || String(usb.portName || "").startsWith("tcp://");
  const connectionLabel = isTcp ? "Wi-Fi" : "USB";
  const portLabel = usb.connected ? usb.portName || savedUsbPortName || "已连接" : "未连接";
  const connectionChip = {
    className: `dashboard-status-bar__chip dashboard-status-bar__chip--${usb.connected ? "ok" : "warn"}`,
    icon: isTcp ? <Wifi size={14} /> : <Usb size={14} />,
    label: usb.connected ? `${connectionLabel} 已连接` : `${connectionLabel} 未连接`,
  };
  return (
    <div className="dashboard-status-bar">
      <span className="dashboard-status-bar__icon">
        <Monitor size={18} />
      </span>
      <div className="dashboard-status-bar__copy">
        <strong className="dashboard-status-bar__board-id">{binding.boardDeviceId}</strong>
        <span className="dashboard-status-bar__sub">
          {connectionLabel}: {portLabel}
        </span>
      </div>
      <div className="dashboard-status-bar__chips">
        <span className={connectionChip.className}>
          {connectionChip.icon}
          {connectionChip.label}
        </span>
      </div>
      <div className="dashboard-status-bar__route-switch" role="group" aria-label="设备连接方式">
        <button
          type="button"
          className={`dashboard-status-bar__route-btn${routePreference === "auto" ? " is-active" : ""}`}
          onClick={() => handleRouteSelect("auto")}
          disabled={switchingRoute}
          title="优先连接 USB；没有 USB 时才连接已保存的 Wi-Fi 地址"
        >自动（USB 优先）</button>
        <button
          type="button"
          className={`dashboard-status-bar__route-btn${routePreference === "usb" ? " is-active" : ""}`}
          onClick={() => handleRouteSelect("usb")}
          disabled={switchingRoute}
          title="只连接 USB，不自动切换到 Wi-Fi"
        >USB</button>
        <button
          type="button"
          className={`dashboard-status-bar__route-btn${routePreference === "tcp" ? " is-active" : ""}`}
          onClick={() => handleRouteSelect("tcp")}
          disabled={switchingRoute || !String(tcpHost || "").trim()}
          title="立即通过指定 Wi-Fi 地址连接"
        >Wi-Fi</button>
      </div>
      <label className="dashboard-status-bar__wifi" title="输入设备当前局域网 IP；连接前会校验板卡身份">
        <Wifi size={14} />
        <input
          type="text"
          value={tcpHost}
          onChange={(event) => {
            setTcpHostDraft({
              boardDeviceId,
              value: event.target.value,
              edited: true,
            });
          }}
          aria-label="设备 Wi-Fi 地址"
          disabled={switchingRoute}
        />
      </label>
    </div>
  );
}
