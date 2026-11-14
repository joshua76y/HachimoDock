"""Verify that the configuration passed to the Wi-Fi driver preserves credentials."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]

class WifiCredentialBoundaryTest(unittest.TestCase):
    def test_full_length_credentials_are_not_truncated(self):
        source = (ROOT / 'main/pet_p4_wifi_bsp.c').read_text()
        fill = 'static void wifi_fill_config(' + source.rsplit(
            'static void wifi_fill_config(', 1)[1].split('\nbool pet_p4_wifi_has_credentials', 1)[0]
        harness = r'''
#include <assert.h>
#include <string.h>
#include <stdint.h>
#define WIFI_AUTH_OPEN 0
#define WPA3_SAE_PWE_BOTH 0
typedef struct { struct { uint8_t ssid[32], password[64];
  struct { int authmode; } threshold; int sae_pwe_h2e; } sta; } wifi_config_t;
''' + fill + r'''
int main(void) {
  char ssid[33], password[65]; wifi_config_t config;
  memset(ssid, 's', 32); ssid[32] = 0;
  memset(password, 'p', 64); password[64] = 0;
  wifi_fill_config(&config, ssid, password);
  assert(memcmp(config.sta.ssid, ssid, 32) == 0);
  assert(memcmp(config.sta.password, password, 64) == 0);
  wifi_fill_config(&config, "abc", "");
  assert(memcmp(config.sta.ssid, "abc\0", 4) == 0);
  assert(config.sta.password[0] == 0);
  return 0;
}
'''
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'test.c'
            binary = Path(tmp) / 'test'
            path.write_text(harness)
            subprocess.run(['cc', '-std=c11', '-Wall', '-Wextra', '-Werror',
                            '-fsanitize=address,undefined', str(path), '-o', str(binary)], check=True)
            subprocess.run([str(binary)], check=True)

if __name__ == '__main__':
    unittest.main()
