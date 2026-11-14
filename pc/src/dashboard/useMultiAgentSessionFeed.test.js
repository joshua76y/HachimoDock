/**
 * [Input] Per-Agent P4 session feed reducer actions.
 * [Output] A retained cross-Agent card queue plus the latest focusable activity.
 * [Pos] Test node for multi-Agent dashboard session orchestration.
 */

import test from "node:test";
import assert from "node:assert/strict";
import {
  MULTI_AGENT_SESSION_FEED_INITIAL_STATE,
  multiAgentSessionFeedReducer,
} from "./useMultiAgentSessionFeed.js";

function reduce(state, action) {
  return multiAgentSessionFeedReducer(state, {
    displayEnabled: true,
    dismissedSessions: {},
    ...action,
  });
}

test("a pushed Agent activity focuses its own project without dropping other Agent cards", () => {
  const codex = reduce(MULTI_AGENT_SESSION_FEED_INITIAL_STATE, {
    type: "replace_snapshot",
    agentId: "codex",
    value: [{ id: "board", name: "Port board", state: "working", statusUpdatedAt: 100 }],
    nowMs: 100,
  });
  const next = reduce(codex, {
    type: "apply_event",
    agentId: "claude-code",
    value: {
      id: "review",
      name: "Review layout",
      state: "working",
      statusUpdatedAt: 200,
      displayContent: "已给出建议",
    },
    nowMs: 200,
  });

  assert.deepEqual(next.sessions.map((session) => session.id), [
    "claude-code::review",
    "codex::board",
  ]);
  assert.deepEqual(next.latestActivity, {
    agentId: "claude-code",
    sessionId: "claude-code::review",
    revision: 200,
  });
});

test("metadata snapshots refresh cards but do not steal focus from a pushed Agent event", () => {
  const pushed = reduce(MULTI_AGENT_SESSION_FEED_INITIAL_STATE, {
    type: "apply_event",
    agentId: "workbuddy",
    value: { id: "reply", name: "Customer reply", state: "working", statusUpdatedAt: 400 },
    nowMs: 400,
  });
  const refreshed = reduce(pushed, {
    type: "replace_snapshot",
    agentId: "codex",
    value: [{ id: "docs", name: "Write docs", state: "working", statusUpdatedAt: 500 }],
    nowMs: 500,
  });

  assert.deepEqual(refreshed.latestActivity, pushed.latestActivity);
  assert.deepEqual(refreshed.sessions.map((session) => session.id), [
    "codex::docs",
    "workbuddy::reply",
  ]);
});
