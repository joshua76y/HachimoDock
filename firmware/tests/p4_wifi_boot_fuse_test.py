"""A driver abort during startup must be counted before it can reset the P4."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]

class WifiBootFuseTest(unittest.TestCase):
    def test_crash_during_init_advances_fuse(self):
        source = (ROOT / 'main/pet_p4_wifi_bsp.c').read_text()
        function = 'esp_err_t pet_p4_wifi_start_from_nvs(void) {' + source.split(
            'esp_err_t pet_p4_wifi_start_from_nvs(void) {', 1)[1].split(
            '\nvoid pet_p4_wifi_describe(', 1)[0]
        harness = r'''
#include <assert.h>
#include <stdbool.h>
#include <stdint.h>
#include <stddef.h>

#include <setjmp.h>
#define WIFI_SSID_MAX 33
#define WIFI_PASS_MAX 65
#define WIFI_FAIL_LIMIT 3
#define ESP_OK 0
#define ESP_ERR_NOT_FOUND 1
#define ESP_ERR_INVALID_STATE 2
#define ESP_LOGI(...) ((void)0)
#define ESP_LOGE(...) ((void)0)
typedef int esp_err_t;
static bool s_has_credentials;
static uint8_t saved;
static jmp_buf reboot;
static int init_result;
static int wifi_load_credentials(char *s, size_t sl, char *p, size_t pl) {
  (void)sl; (void)pl; s[0]='s'; s[1]=0; p[0]='p'; p[1]=0; return ESP_OK;
}
static uint8_t wifi_failcnt_load(void) {return saved;}
static void wifi_failcnt_save(uint8_t count) {saved=count;}
static int pet_p4_wifi_init(const char *s, const char *p) {
  (void)s; (void)p; if (init_result == -1) longjmp(reboot, 1); return init_result;
}
''' + function + r'''
int main(void) {
  init_result=-1;
  if (!setjmp(reboot)) {pet_p4_wifi_start_from_nvs(); assert(0);}
  assert(saved==1);
  if (!setjmp(reboot)) {pet_p4_wifi_start_from_nvs(); assert(0);}
  assert(saved==2);
  if (!setjmp(reboot)) {pet_p4_wifi_start_from_nvs(); assert(0);}
  assert(saved==3);
  assert(pet_p4_wifi_start_from_nvs()==ESP_ERR_INVALID_STATE);
  saved=1; init_result=ESP_OK;
  assert(pet_p4_wifi_start_from_nvs()==ESP_OK); assert(saved==0);
  saved=0; init_result=7;
  assert(pet_p4_wifi_start_from_nvs()==7); assert(saved==1);
  return 0;
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
