"""Regression: the aggregate state topic keeps its declared followed Agent."""

from pathlib import Path
import re
import subprocess


ROOT = Path(__file__).resolve().parents[1]


def static_function(source: str, name: str) -> str:
    match = re.search(rf"^static\s+[^\n]+\b{re.escape(name)}\(", source, re.MULTILINE)
    assert match, f"missing {name}"
    start = match.start()
    brace = source.index("{", match.end())
    depth = 0
    for index in range(brace, len(source)):
        if source[index] == "{":
            depth += 1
        elif source[index] == "}":
            depth -= 1
            if depth == 0:
                return source[start : index + 1]
    raise AssertionError(f"unterminated {name}")


def test_active_state_topic_preserves_the_payload_source_as_followed_agent(tmp_path):
    source = (ROOT / "main/pet_p4_protocol.c").read_text()
    handler = static_function(source, "handle_state_topic")
    harness = f'''\
#include <assert.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include "cJSON.h"
#define PET_P4_AGENT_MAX 48
typedef struct {{ char session_title[96]; }} pet_p4_stats_t;
typedef struct {{
  char active_agent[PET_P4_AGENT_MAX];
  char current_state[24], current_event[64], current_status[24], current_status_text[128];
  char current_speech[256], current_title[128], current_session_title[96], current_session_id[64], current_session_agent[PET_P4_AGENT_MAX];
  uint64_t last_update_ms;
  pet_p4_stats_t stats;
}} pet_p4_runtime_state_t;
static const char *runtime_lifecycle(const pet_p4_runtime_state_t *state) {{ return state->current_state; }}
static bool lifecycle_is(const char *value, const char *expected) {{ return strcmp(value, expected) == 0; }}
static const char *json_string(const cJSON *payload, const char *key) {{
  const cJSON *value = cJSON_GetObjectItemCaseSensitive(payload, key);
  return cJSON_IsString(value) ? value->valuestring : "";
}}
static uint64_t json_u64(const cJSON *payload, const char *key, uint64_t fallback) {{
  const cJSON *value = cJSON_GetObjectItemCaseSensitive(payload, key);
  return cJSON_IsNumber(value) ? (uint64_t)value->valuedouble : fallback;
}}
static void copy_text(char *out, size_t size, const char *value) {{ snprintf(out, size, "%s", value ? value : ""); }}
static void pet_p4_stats_update(pet_p4_stats_t *stats, const cJSON *payload, const char *agent) {{ (void)stats; (void)payload; (void)agent; }}
static void pet_p4_miniapp_sync_stats(const pet_p4_stats_t *stats) {{ (void)stats; }}
static void mark_current_session_terminal(pet_p4_runtime_state_t *state, const char *id, const char *lifecycle, unsigned long long now) {{ (void)state; (void)id; (void)lifecycle; (void)now; }}
static void note_remote_lifecycle(pet_p4_runtime_state_t *state, bool was_done) {{ (void)state; (void)was_done; }}
static long long esp_timer_get_time(void) {{ return 0; }}
{handler}
int main(void) {{
  pet_p4_runtime_state_t state = {{0}};
  cJSON *payload = cJSON_Parse("{{\\\"source\\\":\\\"codex\\\",\\\"state\\\":\\\"idle\\\",\\\"activeTopic\\\":true}}");
  assert(payload);
  handle_state_topic(&state, "state/active", payload);
  assert(strcmp(state.active_agent, "codex") == 0);
  cJSON_Delete(payload);
  strcpy(state.current_session_agent, "claude-code");
  payload = cJSON_Parse("{{\\\"source\\\":\\\"codex\\\",\\\"state\\\":\\\"working\\\",\\\"activeTopic\\\":true}}");
  assert(payload);
  handle_state_topic(&state, "state/active", payload);
  assert(strcmp(state.active_agent, "claude-code") == 0);
  cJSON_Delete(payload);
  return 0;
}}
'''
    path = tmp_path / "active_state_source.c"
    executable = tmp_path / "active_state_source"
    path.write_text(harness)
    subprocess.run(
        [
            "cc", "-std=c11", "-Wall", "-Wextra", "-Werror", "-Wno-unused-parameter",
            "-I", str(ROOT / "tests/vendor/cjson"), str(path),
            str(ROOT / "tests/vendor/cjson/cJSON.c"), "-o", str(executable),
        ],
        check=True,
    )
    subprocess.run([str(executable)], check=True)
