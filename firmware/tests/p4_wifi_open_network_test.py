"""An open network saved successfully remains usable after reboot."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]

class WifiOpenNetworkTest(unittest.TestCase):
    def test_load_saved_open_network(self):
        source = (ROOT / 'main/pet_p4_wifi_bsp.c').read_text()
        function = 'static esp_err_t wifi_load_credentials(' + source.split(
            'static esp_err_t wifi_load_credentials(', 1)[1].split(
            '\nstatic void wifi_fill_config(', 1)[0]
        harness = r'''
#include <assert.h>
#include <stddef.h>
#include <string.h>
typedef int esp_err_t;
typedef int nvs_handle_t;
#define ESP_OK 0
#define ESP_ERR_NOT_FOUND 1
#define NVS_READONLY 0
#define WIFI_NVS_NAMESPACE "pet_wifi"
static int nvs_open(const char *name, int mode, nvs_handle_t *h) {
  (void)name; (void)mode; *h=1; return ESP_OK;
}
static int nvs_get_str(nvs_handle_t h, const char *key, char *out, size_t *len) {
  (void)h; const char *v = strcmp(key, "ssid") == 0 ? "open-network" : "";
  assert(*len >= strlen(v)+1); strcpy(out, v); return ESP_OK;
}
static void nvs_close(nvs_handle_t h) {(void)h;}
''' + function + r'''
int main(void) {
  char ssid[33]={0}, pass[65]={0};
  assert(wifi_load_credentials(ssid, sizeof(ssid), pass, sizeof(pass)) == ESP_OK);
  assert(strcmp(ssid, "open-network") == 0 && pass[0] == 0);
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
