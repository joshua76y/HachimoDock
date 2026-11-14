"""Actual C compact pet status; synthetic only, no SDK/board/provider calls."""
from pathlib import Path
import os
import subprocess
ROOT=Path(__file__).resolve().parents[1]
PRODUCTION=Path(os.environ.get("PET_FW_STATUS_PRODUCTION_ROOT",ROOT))
def test_compact_pet_status_never_uses_agent_identity(tmp_path):
    binary=tmp_path/"pet-status-only"
    stub=tmp_path/"voice_draft_stub.c"
    stub.write_text("#include <stdbool.h>\n#include <stddef.h>\nbool pet_p4_voice_ptt_draft_active(void){return false;}\nvoid pet_p4_voice_ptt_copy_draft_text(char *out,size_t len){if(len)out[0]=0;}\n")
    subprocess.run(["cc","-std=c11","-Wall","-Wextra","-Werror",
        "-I",str(ROOT/"tests/audio_volume_stubs"),"-I",str(PRODUCTION/"main"),str(ROOT/"main/pet_p4_view.c"),
        str(PRODUCTION/"main/pet_p4_conversation.c"),str(ROOT/"tests/p4_pet_status_test.c"),
        str(stub),"-o",str(binary)],check=True)
    subprocess.run([str(binary)],check=True)
