/**
 * [Input] Read DeviceStatusBar.jsx source.
 * [Output] Static Node coverage that the P4 status bar renders a verified transport state, exposes manual USB rescan plus identity-scoped Wi-Fi switching, and uses the documented class names.
 * [Pos] test node in pc/src/dashboard
 * [Sync] If this file changes, update `pc/src/dashboard/.folder.md`.
 */

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, "DeviceStatusBar.jsx"), "utf8");

test("DeviceStatusBar exports a default React component", () => {
  assert.match(source, /export default function DeviceStatusBar\s*\(/);
});

test("DeviceStatusBar consumes useDeviceContext without local status polling", () => {
  assert.match(source, /useDeviceContext\(/);
  assert.doesNotMatch(source, /usb_get_status/);
  assert.doesNotMatch(source, /check_device_availability/);
});

test("DeviceStatusBar exposes the three manual connection routes", () => {
  assert.match(source, /自动（USB 优先）/);
  assert.match(source, />USB</);
  assert.match(source, /Wi-Fi/);
  assert.match(source, /dashboard-status-bar__route-switch/);
  assert.match(source, /switchConnectionRoute/);
});

test("DeviceStatusBar renders the verified current transport", () => {
  assert.match(source, /connectionChip/);
  assert.match(source, /connectionLabel/);
  assert.match(source, /dashboard-status-bar__chips/);
  assert.doesNotMatch(source, /wifiOnline|wifiChip|WiFi 在线|WiFi 离线/);
});

test("DeviceStatusBar delegates an explicit route preference with the configured Wi-Fi host", () => {
  assert.match(source, /handleRouteSelect/);
  assert.match(source, /routePreference/);
  assert.match(source, /host: tcpHost/);
  assert.match(source, /port: 3333/);
  assert.match(source, /aria-label="设备 Wi-Fi 地址"/);
});

test("DeviceStatusBar initializes the automatic Wi-Fi fallback from the paired device", () => {
  assert.match(source, /binding\?\.tcpHost\s*\|\|\s*window\.localStorage/);
  assert.match(source, /setTcpHostDraft/);
});

test("DeviceStatusBar does not carry a typed Wi-Fi address to a different bound board", () => {
  assert.match(source, /tcpHostDraft\.boardDeviceId\s*===\s*boardDeviceId/);
  assert.match(source, /setTcpHostDraft/);
});

test("DeviceStatusBar reads the board id and tolerates the legacy USB port field", () => {
  assert.match(source, /binding\.boardDeviceId/);
  assert.match(source, /binding\.wifiSsid/);
  assert.match(source, /legacyTransportLabel/);
});

test("DeviceStatusBar uses the documented class names", () => {
  assert.match(source, /className="dashboard-status-bar/);
  assert.match(source, /dashboard-status-bar__chip/);
  assert.match(source, /dashboard-status-bar__route-switch/);
  assert.match(source, /dashboard-status-bar__wifi/);
});
