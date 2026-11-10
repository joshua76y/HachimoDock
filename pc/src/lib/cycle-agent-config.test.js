import test from "node:test";
import assert from "node:assert/strict";
import {
  CYCLE_AGENTS_STORAGE_KEY,
  cycleAgentIds,
  effectiveCheckedIds,
  loadCycleAgentSelection,
  saveCycleAgentSelection,
} from "./cycle-agent-config.js";

function installStorage(initial = {}) {
  const values = new Map(Object.entries(initial));
  globalThis.localStorage = {
    getItem: (key) => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, String(value)),
    removeItem: (key) => values.delete(key),
    clear: () => values.clear(),
  };
  globalThis.window = { dispatchEvent() {} };
  return values;
}

test("never configured: every detected agent participates in the cycle", () => {
  installStorage();
  assert.deepEqual(cycleAgentIds(["workbuddy", "codex", "claude-code"]), [
    "workbuddy",
    "codex",
    "claude-code",
  ]);
  assert.deepEqual([...effectiveCheckedIds(["workbuddy", "codex"])], ["workbuddy", "codex"]);
});

test("unchecked installed agents are never cycled to", () => {
  const values = installStorage({
    [CYCLE_AGENTS_STORAGE_KEY]: JSON.stringify(["workbuddy", "codex"]),
  });
  assert.deepEqual(cycleAgentIds(["workbuddy", "codex", "claude-code"]), ["workbuddy", "codex"]);
  assert.deepEqual([...effectiveCheckedIds(["workbuddy", "codex", "claude-code"])], [
    "workbuddy",
    "codex",
  ]);
  values.clear();
});

test("saved ids that are no longer detected are dropped", () => {
  installStorage({
    [CYCLE_AGENTS_STORAGE_KEY]: JSON.stringify(["workbuddy", "ghost-agent"]),
  });
  assert.deepEqual(cycleAgentIds(["workbuddy", "codex"]), ["workbuddy"]);
});

test("save keeps order, removes duplicates, and reloads", () => {
  installStorage();
  assert.equal(loadCycleAgentSelection(), null);
  assert.deepEqual(saveCycleAgentSelection(["codex", "workbuddy", "codex", ""]), [
    "codex",
    "workbuddy",
  ]);
  assert.deepEqual(loadCycleAgentSelection(), ["codex", "workbuddy"]);
});
