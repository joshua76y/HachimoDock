"""Compile the real view model and guard Session-independent recording rendering."""
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def voice_draft_stub(tmp):
    stub = Path(tmp) / "voice_draft_stub.c"
    stub.write_text("#include <stdbool.h>\n#include <stddef.h>\n"
                    "bool pet_p4_voice_ptt_draft_active(void){return false;}\n"
                    "void pet_p4_voice_ptt_copy_draft_text(char *out,size_t len){if(len)out[0]=0;}\n")
    return stub


def test_voice_view_lifecycle():
    with tempfile.TemporaryDirectory() as tmp:
        binary = Path(tmp) / "p4-voice-view-test"
        subprocess.run([
            "cc", "-std=c11", "-Wall", "-Wextra", "-Werror",
            "-I", str(ROOT / "tests/audio_volume_stubs"),
            "-I", str(ROOT / "main"),
            str(ROOT / "main/pet_p4_view.c"),
            str(ROOT / "main/pet_p4_conversation.c"), str(voice_draft_stub(tmp)),
            str(ROOT / "tests/p4_voice_view_test.c"), "-o", str(binary),
        ], check=True)
        subprocess.run([str(binary)], check=True)


def test_display_only_voice_notice_is_expiring_and_never_a_confirmable_draft():
    protocol = (ROOT / "main/pet_p4_protocol.c").read_text()
    renderer = (ROOT / "main/pet_p4_renderer.c").read_text()
    inputs = (ROOT / "main/pet_p4_input.c").read_text()
    assert 'cJSON_IsTrue(cJSON_GetObjectItemCaseSensitive(payload, "voiceNotice"))' in protocol
    assert 'state->voice_notice_until_ms = ' in protocol
    assert 'now_ms >= state->voice_notice_until_ms' in protocol
    assert 'view->show_voice_notice' in renderer
    assert "state->voice_notice_text[0] = '\\0'" in inputs


def test_queue_reset_does_not_end_physical_voice_hold():
    protocol = (ROOT / "main/pet_p4_protocol.c").read_text()
    reset = protocol.split("static void restore_idle_session_view(", 1)[1].split(
        "static void mark_current_session_terminal(", 1)[0]
    assert "session_voice_active = false" not in reset
    source = (ROOT / "main/pet_p4_input.c").read_text()
    clear = source.split('strcmp(binding->action, "session_clear") == 0', 1)[1].split(
        'strcmp(binding->action, "session_next")', 1)[0]
    assert "session_voice_active = false" not in clear
    assert "session_overlay_dismissed = true" in clear
    assert 'strcmp(gesture, "hold_end") == 0' in source
    assert "state->session_voice_active = false" in source


def test_conversation_captions_follow_audio_and_isolate_agent_state():
    with tempfile.TemporaryDirectory() as tmp:
        binary = Path(tmp) / "p4-conversation-test"
        subprocess.run([
            "cc", "-std=c11", "-Wall", "-Wextra", "-Werror",
            "-I", str(ROOT / "tests/audio_volume_stubs"),
            "-I", str(ROOT / "main"),
            str(ROOT / "main/pet_p4_view.c"),
            str(ROOT / "main/pet_p4_conversation.c"), str(voice_draft_stub(tmp)),
            str(ROOT / "tests/p4_conversation_test.c"), "-o", str(binary),
        ], check=True)
        subprocess.run([str(binary)], check=True)


def test_realtime_renderer_and_input_never_use_agent_queue():
    renderer = (ROOT / "main/pet_p4_renderer.c").read_text()
    assert "show_session_queue = !realtime && state && state->session_queue_count > 0" in renderer
    assert "if (!realtime) draw_session_queue(state, now_ms);" in renderer
    assert "realtime ? rgb565(42, 119, 225)" in renderer
    inputs = (ROOT / "main/pet_p4_input.c").read_text()
    dispatch = inputs.split("static void dispatch_binding_event(", 1)[1]
    assert dispatch.index("pet_p4_conversation_active(state)") < dispatch.index("active_global_priority_binding(state, event_name)")
    touch = (ROOT / "main/pet_p4_touch.c").read_text()
    assert "pet_p4_conversation_move(state" in touch
    main = (ROOT / "main/pet_p4_main.c").read_text()
    view_log = main.split('"view page=', 1)[1].split("last_logged_update =", 1)[0]
    assert "view.title" not in view_log
    assert "view.body" not in view_log


def test_recording_row_is_shared_without_duplicate_idle_bubble():
    source = (ROOT / "main/pet_p4_renderer.c").read_text()
    assert "if (view && view->show_voice_overlay) draw_standalone_voice_overlay();" in source
    assert "else if (!show_session_queue) {" in source
    assert "draw_voice_input_row(x, y + 18)" in source
    assert "draw_voice_input_row(x, y + 62)" in source
    assert source.count('draw_text_line_vcenter("LISTENING"') == 1
