/**
 * [Input] useDeviceContext for binding/usb/rescanUsbDevices; Tauri connection commands; useToast for manual result notices.
 * [Output] Region 1 of the P4 dashboard: single-row card showing board id, verified transport state, a manual USB rescan, and an identity-scoped Wi-Fi switch.
 * [Pos] component node in pc/src/dashboard
 * [Sync] If this file changes, update `pc/src/dashboard/.folder.md`.
 */

import React, { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Monitor, RefreshCw, Usb, Wifi } from "lucide-react";
import { useDeviceContext } from "../shell/DeviceContext.jsx";
import { useToast } from "../shell/ToastStack.jsx";

export default function DeviceStatusBar() {
  const { binding, usb, rescanUsbDevices } = useDeviceContext();
  const { push } = useToast();
  const [scanningUsb, setScanningUsb] = useState(false);
  const [tcpHost, setTcpHost] = useState(() => {
    try {
      return String(window.localStorage.getItem("pet-manager:tcp-host") || "192.168.1.97").trim();
    } catch {
      return "192.168.1.97";
    }
  });
  const [switchingWifi, setSwitchingWifi] = useState(false);

  const handleRescanUsb = async () => {
    setScanningUsb(true);
    try {
      const result = await rescanUsbDevices();
      if (result?.status?.connected) {
        const isTcp = result.status.transport === "tcp"
          || String(result.status.portName || "").startsWith("tcp://");
        push({
          tone: "success",
          title: `已连接${isTcp ? " Wi-Fi" : " USB 串口"} ${result.status.portName || ""}`.trim(),
        });
      } else if (result?.devices?.length) {
        push({
          tone: "warning",
          title: "已扫描到 USB 串口，但连接未完成",
          message: result.devices.map((device) => device.portName).join(" / "),
        });
      } else {
        push({
          tone: "warning",
          title: "未检测到 USB 串口设备",
          message: "请确认设备端 USB gadget 已启动，或重新插拔线缆后再扫描。",
        });
      }
    } catch (err) {
      push({
        tone: "error",
        title: "USB 串口扫描失败",
        message: String(err?.message || err),
      });
    } finally {
      setScanningUsb(false);
    }
  };

  const handleConnectWifi = async () => {
    const host = String(tcpHost || "").trim();
    const boardDeviceId = String(binding?.boardDeviceId || "").trim();
    if (!host || !boardDeviceId) return;
    setSwitchingWifi(true);
    try {
      const status = await invoke("usb_connect_tcp", {
        host,
        port: 3333,
        expectedBoardDeviceId: boardDeviceId,
      });
      if (!status?.connected || status?.boardDeviceId !== boardDeviceId) {
        throw new Error("Wi-Fi 已建立连接，但设备身份未通过校验");
      }
      const desktopDeviceId = await invoke("get_or_create_desktop_device_id");
      await invoke("save_device_binding", {
        binding: {
          boardDeviceId,
          desktopDeviceId,
          wifiSsid: `TCP(tcp://${host}:3333)`,
          boundAt: Date.now(),
          transport: "tcp",
          tcpHost: host,
          tcpPort: 3333,
        },
      });
      try { window.localStorage.setItem("pet-manager:tcp-host", host); } catch { /* optional */ }
      push({ tone: "success", title: `已通过 Wi-Fi 连接 ${host}:3333` });
    } catch (error) {
      push({
        tone: "error",
        title: "Wi-Fi 连接失败",
        message: String(error?.message || error),
      });
    } finally {
      setSwitchingWifi(false);
    }
  };

  if (!binding) return null;

  const legacyTransportLabel = String(binding.wifiSsid || "").trim();
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
      <label className="dashboard-status-bar__wifi" title="输入设备当前局域网 IP；连接前会校验板卡身份">
        <Wifi size={14} />
        <input
          type="text"
          value={tcpHost}
          onChange={(event) => setTcpHost(event.target.value)}
          aria-label="设备 Wi-Fi 地址"
          disabled={switchingWifi}
        />
      </label>
      <button
        type="button"
        className="dashboard-status-bar__scan-btn"
        onClick={handleConnectWifi}
        disabled={switchingWifi || !String(tcpHost || "").trim()}
        title="连接到指定 Wi-Fi 地址，并核验保存的设备身份"
      >
        <Wifi size={14} className={switchingWifi ? "spin" : undefined} />
        {switchingWifi ? "连接中..." : "Wi-Fi 直连"}
      </button>
      <button
        type="button"
        className="dashboard-status-bar__scan-btn"
        onClick={handleRescanUsb}
        disabled={scanningUsb}
        title="重新扫描 USB 串口设备"
      >
        <RefreshCw size={14} className={scanningUsb ? "spin" : undefined} />
        {scanningUsb ? "扫描中..." : "重新扫描串口"}
      </button>
    </div>
  );
}
