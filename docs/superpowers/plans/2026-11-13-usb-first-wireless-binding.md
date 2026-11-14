# USB-First Wireless Binding Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Require one verified USB hello before a P4 can be saved or used as a wireless TCP target, while retaining the existing zero-extra-round-trip TCP hello identity check.

**Architecture:** The desktop keeps `boardDeviceId` as the sole persisted board identity; it is the firmware's Base-MAC-derived identifier. USB is the only creation authority for a binding. TCP requires an existing expected identity, validates the existing hello against it, and may only refresh that same binding's endpoint.

**Tech Stack:** Rust/Tauri desktop backend, React frontend, existing Node and Rust test runners.

**Spec:** `docs/superpowers/specs/2026-11-13-usb-first-wireless-binding-design.md`

## Global Constraints

- Do not modify ESP32-P4 firmware, TCP JSONL, hello payloads, HTTP, voice packets, or connection timing.
- Treat existing persisted P4 bindings as prior USB pairings; do not invalidate them.
- A new binding can be created only after a verified physical USB hello.
- TCP must receive a non-empty expected `boardDeviceId`; IP/port remain locators only.
- Preserve all unrelated dirty-worktree changes and do not create commits.

## Review Focus

- Unknown board identity: test that a TCP endpoint cannot create a first binding.
- USB persistence failure: test that the command leaves no usable connected route when pairing cannot be recorded.
- Identity mismatch: test that a candidate TCP hello cannot replace an existing live connection or update its binding.
- Existing paired board: test that USB refresh preserves its saved TCP endpoint and TCP can refresh that endpoint.
- Unbinding: test that removal leaves no eligible TCP auto-reconnect target.

---

### Task 1: Make binding creation USB-only in the Rust backend

**Files:**
- Modify: `pc/src-tauri/src/lib.rs:4691-4867`
- Test: `pc/src-tauri/src/lib.rs:15517-15643`

**Interfaces:**
- Consumes: a verified USB `board_device_id`, persisted `Vec<DeviceBinding>`, and an optional existing same-board binding.
- Produces: a persistence helper that distinguishes verified USB creation from existing-binding updates; `save_device_binding` rejects new identities.

- [ ] **Step 1: Write failing Rust tests for USB-only creation and endpoint refresh**

Add focused tests that assert a new binding is rejected by the ordinary save/update path, a verified USB path creates it, and an existing same-board TCP endpoint remains refreshable.

- [ ] **Step 2: Run the focused tests to verify RED**

Run: `cargo test --lib usb_first_wireless -- --nocapture`

Expected: FAIL because ordinary binding persistence still creates unknown identities.

- [ ] **Step 3: Split persistence authority from generic binding update in `lib.rs`**

Implement a minimal internal helper that permits insertion only for the verified USB path. Make `save_device_binding` reject a missing same-board record, while `persist_connected_usb_binding` uses the USB-authorized path and retains the existing TCP endpoint for that board.

- [ ] **Step 4: Run focused Rust tests to verify GREEN**

Run: `cargo test --lib usb_first_wireless -- --nocapture`

Expected: PASS.

### Task 2: Gate Tauri TCP admission on an existing USB identity

**Files:**
- Modify: `pc/src-tauri/src/lib.rs:7954-7987,11858-11920`
- Test: `pc/src-tauri/src/lib.rs` Rust tests near the Task 1 tests

**Interfaces:**
- Consumes: `usb_connect` verified status and `usb_connect_tcp(expected_board_device_id)`.
- Produces: USB connection commands that persist the verified board before reporting success, and TCP commands that reject an absent expected identity before opening a socket.

- [ ] **Step 1: Write failing Rust tests for missing TCP identity and USB pairing persistence**

Assert that the TCP-command admission helper rejects an empty expected board ID, and that the USB command's pairing path is invoked before it reports a usable connected result.

- [ ] **Step 2: Run the focused tests to verify RED**

Run: `cargo test --lib usb_first_tcp -- --nocapture`

Expected: FAIL because `usb_connect_tcp` currently passes `None` to the transport.

- [ ] **Step 3: Enforce expected identity before TCP probe and pair during USB connect**

Require a trimmed `expected_board_device_id` in `usb_connect_tcp` and pass it to `connect_tcp_expected`. After `usb_connect` completes its verified hello, persist the USB binding before calling downstream connection consumers; if persistence fails, disconnect and return the persistence error.

- [ ] **Step 4: Run focused Rust tests to verify GREEN**

Run: `cargo test --lib usb_first_tcp -- --nocapture`

Expected: PASS.

### Task 3: Keep first-run setup USB-only and preserve paired Wi-Fi updates

**Files:**
- Modify: `pc/src/DeviceSetup.jsx:10-190`
- Modify: `pc/src/DeviceSetup.test.js:37-65`
- Verify unchanged behavior: `pc/src/dashboard/DeviceStatusBar.jsx:62-101`
- Test: `pc/src/dashboard/DeviceStatusBar.test.js`

**Interfaces:**
- Consumes: a backend that persists on verified USB connect and rejects TCP without `expectedBoardDeviceId`.
- Produces: a first-run UI that requests a data USB cable; the dashboard's existing bound-board Wi-Fi update path remains the only manual TCP entry.

- [ ] **Step 1: Write the failing setup policy test**

Replace the test that expects initial Wi-Fi fallback with assertions that the first-run setup has no unscoped `usb_connect_tcp` call or TCP-host persistence, while the dashboard path continues to supply `expectedBoardDeviceId`.

- [ ] **Step 2: Run the setup tests to verify RED**

Run: `node --test src/DeviceSetup.test.js src/dashboard/DeviceStatusBar.test.js`

Expected: FAIL because `DeviceSetup.jsx` still accepts an unpaired TCP connection.

- [ ] **Step 3: Remove initial Wi-Fi binding from `DeviceSetup.jsx`**

Delete the TCP-host state, fallback callback, and generic binding save from the first-run wizard. Keep USB scanning, recovery scanning, successful handoff, and the user-facing instruction to use a data-capable USB cable. Leave the dashboard's identity-scoped Wi-Fi address update unchanged.

- [ ] **Step 4: Run the setup tests to verify GREEN**

Run: `node --test src/DeviceSetup.test.js src/dashboard/DeviceStatusBar.test.js`

Expected: PASS.

### Task 4: Verify the complete desktop binding contract

**Files:**
- Verify: `pc/src-tauri/src/lib.rs`
- Verify: `pc/src/DeviceSetup.jsx`
- Verify: `pc/src/dashboard/DeviceStatusBar.jsx`

- [ ] **Step 1: Run targeted native and frontend regressions**

Run:
```bash
cargo test --lib usb_first_wireless -- --nocapture
cargo test --lib usb_first_tcp -- --nocapture
node --test src/DeviceSetup.test.js src/dashboard/DeviceStatusBar.test.js src/ProductExperience.test.js
```

Expected: all selected tests PASS.

- [ ] **Step 2: Run project-level checks and report unrelated known failures honestly**

Run:
```bash
npm test
cargo test --lib
```

Expected: frontend suite passes; if the known unrelated Rust USB lifecycle or persona-LLM failures recur, report them by test name and do not claim a clean full Rust suite.

- [ ] **Step 3: Perform manual USB-to-Wi-Fi acceptance without firmware changes**

Confirm on the connected P4: existing paired board reconnects over TCP; unbinding prevents wireless reconnection; USB re-pairing restores it. Do not send or record voice content during this check.
