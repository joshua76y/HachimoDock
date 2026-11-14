"""The device must allow the desktop's documented ASR timeout plus transfer time."""
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]

def test_device_waits_longer_than_desktop_asr_budget():
    device = (ROOT / 'main/pet_p4_voice_ptt.c').read_text()
    desktop = (ROOT.parent / 'pc/src-tauri/src/device_voice_bridge.rs').read_text()
    device_ms = int(re.search(r'#define PTT_POST_TIMEOUT_MS\s+(\d+)', device).group(1))
    desktop_s = int(re.search(r'const ASR_TIMEOUT: Duration = Duration::from_secs\((\d+)\)', desktop).group(1))
    # The desktop may spend 20 seconds starting the service before the ASR call.
    assert device_ms >= (desktop_s + 20 + 5) * 1000
