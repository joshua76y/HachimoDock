"""Use the last connected host when the optional NVS namespace does not exist."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]

class VoiceWifiHostFallbackTest(unittest.TestCase):
    def test_absent_namespace_uses_peer(self):
        source = (ROOT / 'main/pet_p4_voice_wifi.c').read_text()
        function = 'esp_err_t pet_p4_voice_wifi_start_configured(void) {' + source.split(
            'esp_err_t pet_p4_voice_wifi_start_configured(void) {', 1)[1]
        harness = r'''
#include <assert.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>
typedef int esp_err_t;
typedef int nvs_handle_t;
#define ESP_OK 0
#define ESP_ERR_NOT_FOUND 1
#define NVS_READONLY 0
#define PET_P4_VOICE_WIFI_PORT 8898
static char s_peer_host[32] = "192.0.2.10";
static char used[32];
static int has_config;
static bool tcp_connected = true;
bool pet_p4_tcp_client_connected(void) {return tcp_connected;}
static bool pet_p4_voice_wifi_copy_host(char *out, size_t len) {
  if (!s_peer_host[0] || strlen(s_peer_host) >= len) return false;
  strcpy(out, s_peer_host); return true;
}
static int nvs_open(const char *ns, int mode, nvs_handle_t *h) {
  (void)ns; (void)mode; *h = 1; return has_config ? ESP_OK : ESP_ERR_NOT_FOUND;
}
static int nvs_get_str(nvs_handle_t h, const char *key, char *out, size_t *len) {
  (void)h; (void)key; assert(*len >= sizeof("192.0.2.20"));
  strcpy(out, "192.0.2.20"); return ESP_OK;
}
static void nvs_close(nvs_handle_t h) {(void)h;}
static int pet_p4_voice_wifi_start(const char *host, uint16_t port) {
  assert(port == PET_P4_VOICE_WIFI_PORT); strcpy(used, host); return ESP_OK;
}
''' + function + r'''
int main(void) {
  assert(pet_p4_voice_wifi_start_configured() == ESP_OK);
  assert(strcmp(used, s_peer_host) == 0);
  has_config = 1;
  assert(pet_p4_voice_wifi_start_configured() == ESP_OK);
  assert(strcmp(used, "192.0.2.20") == 0);
  has_config = 0;
  tcp_connected = false;
  assert(pet_p4_voice_wifi_start_configured() == ESP_ERR_NOT_FOUND);
  tcp_connected = true;
  s_peer_host[0] = 0;
  assert(pet_p4_voice_wifi_start_configured() == ESP_ERR_NOT_FOUND);
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
