"""Ending capture completes or aborts the local PTT buffer exactly once."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]

class VoiceWifiCaptureEndTest(unittest.TestCase):
    def test_capture_end_completes_or_aborts_local_ptt(self):
        source = (ROOT / 'main/pet_p4_audio.c').read_text()
        function = 'static void send_audio_end(' + source.split(
            'static void send_audio_end(', 1)[1].split(
            '\nstatic uint16_t read_le16(', 1)[0]
        harness = r'''
#include <assert.h>
#include <stdbool.h>
#include <stdint.h>
#include <string.h>
#include <stdio.h>
#define PET_P4_AUDIO_SAMPLE_RATE 16000
#define PET_P4_AUDIO_CHANNELS 1
#define PET_P4_AUDIO_BITS_PER_SAMPLE 16
typedef int cJSON;
static int ptt_completed, ptt_aborted;
static bool pet_p4_voice_ptt_capturing(void) {return false;}
static void pet_p4_voice_ptt_notify_end(void) {ptt_completed++;}
void pet_p4_voice_ptt_abort(void) {ptt_aborted++;}
static cJSON *cJSON_CreateObject(void) {static int obj; return &obj;}
static void cJSON_AddStringToObject(cJSON *o, const char *k, const char *v) {(void)o;(void)k;(void)v;}
static void cJSON_AddNumberToObject(cJSON *o, const char *k, double v) {(void)o;(void)k;(void)v;}
static void send_topic(const char *topic, cJSON *payload) {(void)topic;(void)payload;}
''' + function + r'''
int main(void) {
  send_audio_end("one", "released", 1, 320, 0);
  send_audio_end("two", "released", 1, 320, 0);
  send_audio_end("bad", "read_error", 1, 320, 0);
  assert(ptt_completed == 2 && ptt_aborted == 1);
}
'''
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'test.c'
            binary = Path(tmp) / 'test'
            path.write_text(harness)
            subprocess.run(['cc', '-std=c11', '-Wall', '-Wextra', '-Werror',
                            str(path), '-o', str(binary)], check=True)
            subprocess.run([str(binary)], check=True)

if __name__ == '__main__':
    unittest.main()
