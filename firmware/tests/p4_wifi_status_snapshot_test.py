"""A concurrent IP update must not produce a mixed address for a reader."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT=Path(__file__).resolve().parents[1]

class WifiStatusSnapshotTest(unittest.TestCase):
    def test_ip_read_is_one_coherent_snapshot(self):
        source=(ROOT/'main/pet_p4_wifi_bsp.c').read_text()
        functions='bool pet_p4_wifi_is_connected(void) {'+source.split('bool pet_p4_wifi_is_connected(void) {',1)[1].split('\n\n/* ── 配网',1)[0]
        harness=r'''
#include <assert.h>
#include <stdbool.h>
#include <stddef.h>
#include <string.h>
#define ESP_OK 0
#define ESP_ERR_INVALID_STATE 1
typedef int esp_err_t;
static bool s_connected=true;
static char s_ip[32]="111.111.111.111";
static int s_status_lock, depth, deferred, injected;
static void update(void){memcpy(s_ip,"222.222.222.222",16);}
static void enter(int *l){(void)l;depth++;}
static void leave(int *l){(void)l;depth--;if(!depth&&deferred){deferred=0;update();}}
#define taskENTER_CRITICAL(l) enter(l)
#define taskEXIT_CRITICAL(l) leave(l)
static char *test_strncpy(char *d,const char *s,size_t n){
  size_t i=0;for(;i<n;i++){if(i==4&&!injected){injected=1;if(depth)deferred=1;else update();} d[i]=s[i];if(!s[i]){i++;break;}}
  for(;i<n;i++)d[i]=0;return d;
}
#undef strncpy
#define strncpy test_strncpy
''' + functions + r'''
int main(void){char out[32];assert(pet_p4_wifi_is_connected());
  assert(pet_p4_wifi_get_ip(out,sizeof(out))==ESP_OK);
  assert(strcmp(out,"111.111.111.111")==0);
  assert(strcmp(s_ip,"222.222.222.222")==0);
  s_connected=false;assert(pet_p4_wifi_get_ip(out,sizeof(out))==ESP_ERR_INVALID_STATE);
  assert(pet_p4_wifi_get_ip(NULL,0)==ESP_ERR_INVALID_STATE);
}
'''
        with tempfile.TemporaryDirectory() as tmp:
            path=Path(tmp)/'test.c';binary=Path(tmp)/'test';path.write_text(harness)
            subprocess.run(['cc','-std=c11','-Wall','-Wextra','-Werror','-Wno-unused-function','-Wno-unused-variable',str(path),'-o',str(binary)],check=True)
            subprocess.run([str(binary)],check=True,timeout=15)
if __name__=='__main__':unittest.main()
