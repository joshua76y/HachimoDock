"""Exercise actual PTT capture admission and end-of-recording ownership."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]

class VoicePttOwnershipTest(unittest.TestCase):
    def test_pending_clip_is_not_overwritten_and_short_clip_releases_lease(self):
        source = (ROOT / 'main/pet_p4_voice_ptt.c').read_text()
        functions = 'pet_p4_voice_ptt_begin_result_t pet_p4_voice_ptt_begin(' + source.split(
            'pet_p4_voice_ptt_begin_result_t pet_p4_voice_ptt_begin(', 1)[1].split(
            '\n/* ── WAV 封装', 1)[0]
        harness = r'''
#include <assert.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#define PTT_BUF_BYTES (512 * 1024)
#define MALLOC_CAP_SPIRAM 0
#define MALLOC_CAP_8BIT 0
#define ESP_OK 0
#define pdPASS 1
#define PTT_BEGIN_TASK_STACK 4096
#define TAG "test"
#define ESP_LOGW(...) ((void)0)
#define ESP_LOGI(...) ((void)0)
typedef int esp_err_t;
typedef enum {PET_P4_VOICE_PTT_UNAVAILABLE, PET_P4_VOICE_PTT_STARTED, PET_P4_VOICE_PTT_BUSY} pet_p4_voice_ptt_begin_result_t;
static char *g_pcm_buf;
static char g_url[128], g_capture_url[128], g_capture_sid[17];
static atomic_size_t g_pcm_len;
static atomic_bool g_capturing, g_submit_pending, g_ptt_owned, g_pcm_truncated;
static void *g_ptt_task_handle = (void *)1;
static int capture_result;
static bool connected = true;
static bool endpoint_available = true;
static bool pet_p4_wifi_is_connected(void) {return connected;}
static void *heap_caps_malloc(size_t n, int c) {(void)c;return malloc(n);}
static int pet_p4_audio_capture_start_owned(bool q, const char *o) {(void)q;(void)o;return capture_result;}
static const char *esp_err_to_name(int e) {(void)e;return "error";}
static void pet_p4_voice_ptt_get_url(char *out,size_t n) {assert(n>5);strcpy(out,"valid");}
static bool tcp_connected;
static bool pet_p4_tcp_client_connected(void) {return tcp_connected;}
static bool pet_p4_voice_wifi_copy_host(char *out,size_t n) {(void)out;(void)n;return false;}
static bool ptt_resolve_endpoint(const char *c,const char *p,char *o,size_t n) {(void)p;assert(n>strlen(c));strcpy(o,c);return endpoint_available;}
static unsigned esp_random(void) {return 42;}
static char *ptt_begin_url_dup(const char *url,const char *sid) {(void)url;(void)sid;return NULL;}
static void ptt_begin_post_task(void *p) {(void)p;}
static int xTaskCreate(void (*f)(void *), const char *n, int s, void *a, int p, void *h) {(void)f;(void)n;(void)s;(void)a;(void)p;(void)h;return pdPASS;}
static void xTaskNotifyGive(void *h) {(void)h;}
static int shown, progress;
static void ptt_show_text(const char *s, size_t n) {(void)s;(void)n;shown++;}
static void ptt_show_progress(size_t n) {assert(n>=3200);progress++;}
''' + functions + r'''
int main(void) {
  uint8_t pcm[3200]; memset(pcm, 0x5a, sizeof(pcm));
  assert(!atomic_load(&g_pcm_truncated));
  assert(pet_p4_voice_ptt_begin(true)==PET_P4_VOICE_PTT_STARTED);
  pet_p4_voice_ptt_feed(pcm,sizeof(pcm));
  pet_p4_voice_ptt_notify_end();
  assert(atomic_load(&g_submit_pending));
  tcp_connected=true;
  assert(pet_p4_voice_ptt_begin(true)==PET_P4_VOICE_PTT_BUSY);
  tcp_connected=false;
  assert(progress==1);
  assert(pet_p4_voice_ptt_begin(true)==PET_P4_VOICE_PTT_BUSY);
  connected = false;
  assert(pet_p4_voice_ptt_begin(true)==PET_P4_VOICE_PTT_BUSY);
  connected = true;
  assert(atomic_load(&g_pcm_len)==sizeof(pcm));
  assert(memcmp(g_pcm_buf,pcm,sizeof(pcm))==0);
  atomic_store(&g_submit_pending,false); atomic_store(&g_ptt_owned,false);
  tcp_connected=true;
  assert(pet_p4_voice_ptt_begin(true)==PET_P4_VOICE_PTT_UNAVAILABLE);
  assert(!atomic_load(&g_ptt_owned));
  tcp_connected=false;
  assert(pet_p4_voice_ptt_begin(true)==PET_P4_VOICE_PTT_STARTED);
  pet_p4_voice_ptt_feed(pcm,100);
  pet_p4_voice_ptt_notify_end();
  assert(!atomic_load(&g_ptt_owned));
  assert(pet_p4_voice_ptt_begin(true)==PET_P4_VOICE_PTT_STARTED);
  uint8_t *too_long = calloc(PTT_BUF_BYTES + 1, 1); assert(too_long);
  int before = shown;
  pet_p4_voice_ptt_feed(too_long, PTT_BUF_BYTES + 1);
  pet_p4_voice_ptt_notify_end();
  assert(!atomic_load(&g_submit_pending));
  assert(!atomic_load(&g_ptt_owned) && shown == before + 1);
  free(too_long);
  assert(pet_p4_voice_ptt_begin(true)==PET_P4_VOICE_PTT_STARTED);
  pet_p4_voice_ptt_feed(pcm,sizeof(pcm));
  before=shown; pet_p4_voice_ptt_abort();
  assert(!atomic_load(&g_submit_pending) && !atomic_load(&g_ptt_owned));
  assert(shown==before+1);
  endpoint_available=false;
  assert(pet_p4_voice_ptt_begin(true)==PET_P4_VOICE_PTT_UNAVAILABLE);
  assert(!atomic_load(&g_ptt_owned));
  endpoint_available=true;
  capture_result=1;
  assert(pet_p4_voice_ptt_begin(true)==PET_P4_VOICE_PTT_UNAVAILABLE);
  assert(!atomic_load(&g_ptt_owned));
  free(g_pcm_buf);
}
'''
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'test.c'
            binary = Path(tmp) / 'test'
            path.write_text(harness)
            subprocess.run(['cc','-std=c11','-Wall','-Wextra','-Werror',
                            '-Wno-unused-function','-fsanitize=address,undefined',
                            str(path),'-o',str(binary)],check=True)
            subprocess.run([str(binary)],check=True,timeout=15)

if __name__ == '__main__':
    unittest.main()
