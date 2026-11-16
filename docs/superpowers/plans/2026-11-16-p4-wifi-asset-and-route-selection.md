# P4 Wi-Fi Asset and Route Selection Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make P4 appearance installation work over Wi-Fi and give the user a persistent USB-first automatic route with explicit USB/Wi-Fi overrides.

**Architecture:** The serial manager decides the appearance wire encoding from its active connection transport: raw bytes only for physical serial and JSON/Base64 for TCP. The desktop binding persists a separate route preference, while the Rust auto-connector consults that preference before probing USB or the identity-scoped TCP endpoint. The React status bar changes that preference through one backend command and renders the active/desired route.

**Tech Stack:** Rust/Tauri, React, existing P4 JSONL asset protocol, Node and Cargo tests.

**Spec:** `docs/superpowers/specs/2026-11-16-p4-wifi-asset-and-route-selection-design.md`

## Global Constraints

- USB raw asset transfer remains unchanged; TCP uses the already supported bounded JSON/Base64 asset chunk path.
- TCP remains scoped to an existing paired `boardDeviceId`; no new handshake or firmware change.
- `auto` is the default for absent/invalid persisted preferences and always probes USB before TCP.
- Exactly one control connection may be live at a time.

## Review Focus

- TCP 30 KiB JSON line limit: Task 1 retains the existing 20,478-byte JSON chunk fallback and tests TCP disables raw.
- Legacy binding: Task 2 tests an empty preference normalizes to `auto`.
- USB available in auto mode: Task 2 tests route policy forbids TCP before USB scan succeeds/fails.
- Manual Wi-Fi override with USB connected: Task 3 tests the UI sends an explicit `tcp` preference rather than merely opening a one-off connection.
- Mismatched TCP peer: Task 2 retains `connect_tcp_expected` and the board-ID requirement in every TCP branch.

---

### Task 1: Select a TCP-safe P4 appearance wire format

**Files:**
- Modify: `pc/src-tauri/src/usb_serial.rs:4994-5016,6071-6096`
- Test: `pc/src-tauri/src/usb_serial.rs` near `p4_raw_chunk_policy_uses_platform_limit_and_safe_integrity_fallback`

**Interfaces:**
- Produces: a pure transport policy helper that maps a port name plus raw-capability flag to raw or JSON/Base64 chunks.

- [x] **Step 1: Write a failing unit test** for USB raw enabled, TCP raw disabled, and unsupported raw disabled.
- [x] **Step 2: Run** `cargo test --lib p4_asset_transport -- --nocapture` **and verify RED**.
- [x] **Step 3: Implement the helper and use it in** `sync_appearance_p4_with_stages`, keeping raw-slot handling conditional on raw transport.
- [x] **Step 4: Run the focused test and existing P4 chunk-size test; verify GREEN.**

### Task 2: Persist and apply USB-first route preference

**Files:**
- Modify: `pc/src-tauri/src/lib.rs:4270-4300,4782-4867,8085-8118,11992-12135`
- Test: `pc/src-tauri/src/lib.rs` binding and auto-connect tests

**Interfaces:**
- Produces: `ConnectionRoutePreference` normalization, a binding update command, and an auto-connect policy that orders USB/TCP probes.

- [x] **Step 1: Write failing Rust tests** for default `auto`, preference normalization, and the three probe orders.
- [x] **Step 2: Run** `cargo test --lib route_preference -- --nocapture` **and verify RED**.
- [x] **Step 3: Add the serde-defaulted route preference to `DeviceBinding`; expose a validated Tauri command that updates an existing binding.**
- [x] **Step 4: Refactor auto-connect into preference-based ordering:** auto scans verified USB before TCP, USB skips TCP, TCP skips USB; keep `connect_tcp_expected` and backoff.
- [x] **Step 5: Run focused Rust tests and the existing USB-first binding tests; verify GREEN.**

### Task 3: Provide a manual route switch in the dashboard

**Files:**
- Modify: `pc/src/shell/DeviceContext.jsx:330-390`
- Modify: `pc/src/dashboard/DeviceStatusBar.jsx`
- Modify: `pc/src/styles.css:350-490`
- Test: `pc/src/dashboard/DeviceStatusBar.test.js`, `pc/src/shell/DeviceContext.test.js`

**Interfaces:**
- Consumes: `set_connection_route_preference(route, boardDeviceId, host, port)` and `rescanUsbDevices`.
- Produces: a three-state route selector with immediate transition and status feedback.

- [x] **Step 1: Write failing frontend assertions** for the selector labels, explicit backend preference update, and an auto-mode USB-first rescan.
- [x] **Step 2: Run** `node --test src/dashboard/DeviceStatusBar.test.js src/shell/DeviceContext.test.js` **and verify RED**.
- [x] **Step 3: Implement a context route-switch callback** that selects USB, TCP, or auto and refreshes the shared status; update `DeviceStatusBar` to render compact selectable route controls.
- [x] **Step 4: Run focused frontend tests; verify GREEN.**

### Task 4: Verify end-to-end behavior

**Files:**
- Verify: the Task 1–3 files

- [x] **Step 1: Run targeted Rust and frontend tests.**
- [ ] **Step 2: Build the desktop app and manually verify the connected P4 on Wi-Fi:** switch a pet/appearance and confirm the transfer reaches completion; use the route controls to switch Wi-Fi → USB → auto. (Desktop build completed; hardware acceptance pending.)
- [x] **Step 3: Commit the coherent implementation with tests and both design documents.**
