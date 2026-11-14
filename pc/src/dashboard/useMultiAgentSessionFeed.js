/**
 * [Input] Enabled Agent ids, ordered Session events, snapshots, and dismissed device cards.
 * [Output] One source-scoped P4 card queue; event activity chooses the next Agent/project focus.
 * [Pos] Multi-Agent Session polling boundary for the persistent device dashboard.
 */

import { useEffect, useMemo, useReducer } from "react";
import {
  fetchAgentSessionEvents,
  fetchAgentSessions,
} from "../lib/agent-bus-client.js";
import {
  buildP4MultiAgentRoutingSessions,
  parseP4ScopedSessionId,
} from "../lib/p4-session-service.js";
import {
  AGENT_SESSION_FEED_INITIAL_STATE,
  agentSessionFeedReducer,
  P4_SESSION_EVENT_DISPATCH_GAP_MS,
  P4_SESSION_EVENT_POLL_MS,
  P4_SESSION_SNAPSHOT_POLL_MS,
} from "./useAgentSessionFeed.js";

export const MULTI_AGENT_SESSION_FEED_INITIAL_STATE = Object.freeze({
  feeds: Object.freeze({}),
  sessions: Object.freeze([]),
  routingSessions: Object.freeze([]),
  latestActivity: null,
});

function normalizeText(value) {
  return typeof value === "string" ? value.trim() : "";
}

function scopedDismissals(agentId, dismissedSessions) {
  const output = {};
  for (const [key, value] of Object.entries(dismissedSessions || {})) {
    const scoped = parseP4ScopedSessionId(key);
    if (scoped.agentId === agentId && scoped.sessionId) output[scoped.sessionId] = value;
  }
  return output;
}

function flattenFeeds(feeds, key) {
  return buildP4MultiAgentRoutingSessions(
    Object.entries(feeds || {}).map(([agentId, feed]) => ({
      agentId,
      sessions: Array.isArray(feed?.[key]) ? feed[key] : [],
    })),
  );
}

function activityForEvent(agentId, feed, event, nowMs) {
  const sourceSessionId = normalizeText(event?.id);
  const session = (feed?.sessions || []).find((candidate) => candidate?.id === sourceSessionId);
  if (!session) return null;
  const sessionId = `${agentId}::${sourceSessionId}`;
  const revision = Math.max(
    Number(session?.statusUpdatedAt || 0),
    Number(session?.lastModified || 0),
    Number(nowMs || 0),
  );
  return { agentId, sessionId, revision };
}

export function multiAgentSessionFeedReducer(state, action) {
  const agentId = normalizeText(action?.agentId);
  if (!agentId) return state;
  const currentFeed = state.feeds[agentId] || AGENT_SESSION_FEED_INITIAL_STATE;
  const nextFeed = agentSessionFeedReducer(currentFeed, {
    ...action,
    agentId,
    dismissedSessions: scopedDismissals(agentId, action.dismissedSessions),
  });
  if (nextFeed === currentFeed) return state;
  const feeds = { ...state.feeds, [agentId]: nextFeed };
  let latestActivity = state.latestActivity;
  if (action.type === "apply_event") {
    const candidate = activityForEvent(agentId, nextFeed, action.value, action.nowMs);
    if (candidate && (!latestActivity || candidate.revision >= latestActivity.revision)) {
      latestActivity = candidate;
    }
  }
  return {
    feeds,
    sessions: flattenFeeds(feeds, "sessions"),
    routingSessions: flattenFeeds(feeds, "routingSessions"),
    latestActivity,
  };
}

function uniqueAgentIds(agentIds) {
  return [...new Set((Array.isArray(agentIds) ? agentIds : [])
    .map(normalizeText)
    .filter(Boolean))];
}

export function useMultiAgentSessionFeed({ agentIds, displayEnabled, dismissedSessions }) {
  const [state, dispatch] = useReducer(
    multiAgentSessionFeedReducer,
    MULTI_AGENT_SESSION_FEED_INITIAL_STATE,
  );
  const monitoredAgentIds = useMemo(() => uniqueAgentIds(agentIds), [JSON.stringify(agentIds || [])]);
  const agentIdsSignature = monitoredAgentIds.join(",");
  const sessions = state.sessions;
  const routingSessions = state.routingSessions;
  const loaded = monitoredAgentIds.length > 0
    && monitoredAgentIds.every((agentId) => state.feeds[agentId]?.loaded === true);

  useEffect(() => {
    if (monitoredAgentIds.length === 0) return undefined;
    let cancelled = false;
    const ctl = new AbortController();
    const timers = [];
    const cleanupTimers = () => timers.splice(0).forEach(clearTimeout);

    for (const agentId of monitoredAgentIds) {
      let cursor;
      let streamId = "";
      let eventsInFlight = false;
      let snapshotInFlight = false;
      let draining = false;
      const pendingEvents = [];

      const tickTerminal = () => dispatch({
        type: "tick_terminal", agentId, nowMs: Date.now(), displayEnabled, dismissedSessions,
      });
      const drainEvents = () => {
        if (cancelled) { draining = false; return; }
        const event = pendingEvents.shift();
        if (!event) { draining = false; return; }
        if (event.session && normalizeText(event.session.id)) {
          dispatch({
            type: "apply_event",
            agentId,
            value: event.session,
            nowMs: Number(event.updatedAt || event.session.statusUpdatedAt || Date.now()),
            displayEnabled,
            dismissedSessions,
          });
        }
        timers.push(setTimeout(drainEvents, P4_SESSION_EVENT_DISPATCH_GAP_MS));
      };
      const enqueueEvents = (events) => {
        pendingEvents.push(...(Array.isArray(events) ? events : []));
        if (draining || pendingEvents.length === 0) return;
        draining = true;
        drainEvents();
      };
      const pollEvents = async () => {
        if (cancelled || eventsInFlight) return;
        eventsInFlight = true;
        try {
          const result = await fetchAgentSessionEvents(agentId, cursor, streamId, ctl.signal);
          if (cancelled) return;
          if (result.reset || (streamId && result.streamId && result.streamId !== streamId)) {
            pendingEvents.length = 0;
          }
          cursor = result.cursor;
          streamId = result.streamId;
          enqueueEvents(result.events);
        } catch {
          // Preserve the last confirmed queue; the next loop retries the cursor.
        } finally {
          eventsInFlight = false;
          tickTerminal();
        }
      };
      const refreshSnapshot = async () => {
        if (cancelled || snapshotInFlight) return;
        snapshotInFlight = true;
        try {
          const nextSessions = await fetchAgentSessions(agentId, ctl.signal);
          if (!cancelled) {
            dispatch({
              type: "replace_snapshot", agentId, value: nextSessions, nowMs: Date.now(),
              displayEnabled, dismissedSessions,
            });
          }
        } catch {
          // Event polling remains an independent source of live activity.
        } finally {
          snapshotInFlight = false;
        }
      };
      const scheduleEvents = () => timers.push(setTimeout(async () => {
        await pollEvents();
        if (!cancelled) scheduleEvents();
      }, P4_SESSION_EVENT_POLL_MS));
      const scheduleSnapshots = () => timers.push(setTimeout(async () => {
        await refreshSnapshot();
        if (!cancelled) scheduleSnapshots();
      }, P4_SESSION_SNAPSHOT_POLL_MS));
      (async () => {
        await pollEvents();
        await refreshSnapshot();
        await pollEvents();
        if (!cancelled) { scheduleEvents(); scheduleSnapshots(); }
      })();
    }
    return () => {
      cancelled = true;
      ctl.abort();
      cleanupTimers();
    };
  }, [agentIdsSignature, dismissedSessions, displayEnabled]);

  return { sessions, routingSessions, loaded, latestActivity: state.latestActivity };
}
