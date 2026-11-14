"""Exercise the device's draft decision mapping with the actual C helper."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class VoiceDraftGestureTest(unittest.TestCase):
    def test_board_wiring_maps_measured_vertical_axis_to_logical_y(self):
        source = (ROOT / "main/pet_p4_input.c").read_text()
        self.assertIn("#define PET_P4_INPUT_JOYSTICK_X_GPIO GPIO_NUM_22", source)
        self.assertIn("#define PET_P4_INPUT_JOYSTICK_Y_GPIO GPIO_NUM_21", source)
        self.assertIn("#define PET_P4_INPUT_JOYSTICK_X_CHANNEL ADC1_GPIO22_CHANNEL", source)
        self.assertIn("#define PET_P4_INPUT_JOYSTICK_Y_CHANNEL ADC1_GPIO21_CHANNEL", source)

    def test_only_vertical_joystick_decides_and_has_input_priority(self):
        source = (ROOT / "main/pet_p4_input.c").read_text()
        function = "static const char *voice_draft_action_for_event(" + source.split(
            "static const char *voice_draft_action_for_event(", 1)[1].split("\n}\n", 1)[0] + "\n}\n"
        dispatch = source.split("static void dispatch_binding_event(", 1)[1]
        self.assertLess(dispatch.index("voice_draft_action_for_event(event_name)"),
                        dispatch.index("pet_p4_conversation_active(state)"))
        harness = '#include <assert.h>\n#include <string.h>\n' + function + r'''
int main(void) {
  assert(!strcmp(voice_draft_action_for_event("joystick.up"), "voice_draft_send"));
  assert(!strcmp(voice_draft_action_for_event("joystick.down"), "voice_draft_cancel"));
  assert(voice_draft_action_for_event("knob.rotate_ccw") == 0);
  assert(voice_draft_action_for_event("knob.rotate_cw") == 0);
  assert(voice_draft_action_for_event("button.1.short_press") == 0);
  assert(voice_draft_action_for_event(0) == 0);
}
'''
        with tempfile.TemporaryDirectory() as tmp:
            path, binary = Path(tmp) / "test.c", Path(tmp) / "test"
            path.write_text(harness)
            subprocess.run(["cc", "-std=c11", "-Wall", "-Wextra", "-Werror",
                            str(path), "-o", str(binary)], check=True)
            subprocess.run([str(binary)], check=True, timeout=15)


if __name__ == "__main__":
    unittest.main()
