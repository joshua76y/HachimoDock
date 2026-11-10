/**
 * [Input] Detected agent ids and a persisted checkbox selection in localStorage.
 * [Output] The ordered set of agents that device-side agent_next/agent_previous
 *          cycling is allowed to visit. Unchecked (but installed) agents are
 *          never cycled to, per user requirement.
 * [Pos] lib node in pc/src/lib
 */
export const CYCLE_AGENTS_STORAGE_KEY = "pet-manager.cycle-agents";
export const CYCLE_AGENTS_UPDATED_EVENT = "cycle-agents-updated";

export function loadCycleAgentSelection() {
  try {
    const raw = localStorage.getItem(CYCLE_AGENTS_STORAGE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed.filter((id) => typeof id === "string" && id) : null;
  } catch {
    return null;
  }
}

export function saveCycleAgentSelection(ids) {
  const ordered = [...new Set((ids || []).filter((id) => typeof id === "string" && id))];
  try {
    localStorage.setItem(CYCLE_AGENTS_STORAGE_KEY, JSON.stringify(ordered));
  } catch { /* storage may be unavailable in a restricted context */ }
  try {
    window.dispatchEvent(new Event(CYCLE_AGENTS_UPDATED_EVENT));
  } catch { /* non-browser unit-test context */ }
  return ordered;
}

/**
 * Selection semantics:
 * - Never configured: every detected agent is checked (opt-out, zero setup).
 * - Configured: keep the user's checked ids in their saved order, but drop
 *   ids that are no longer detected on this machine.
 * - A newly installed agent is NOT silently added to the cycle list; the user
 *   checks it explicitly in the PC panel.
 */
export function cycleAgentIds(detectedIds = [], savedIds = loadCycleAgentSelection()) {
  const detected = (detectedIds || []).filter((id) => typeof id === "string" && id);
  if (!savedIds) return detected;
  const detectedSet = new Set(detected);
  return savedIds.filter((id) => detectedSet.has(id));
}

export function effectiveCheckedIds(detectedIds = []) {
  const saved = loadCycleAgentSelection();
  const detected = (detectedIds || []).filter((id) => typeof id === "string" && id);
  if (!saved) return new Set(detected);
  const detectedSet = new Set(detected);
  return new Set(saved.filter((id) => detectedSet.has(id)));
}
