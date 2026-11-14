"""Device Wi-Fi audio frames must carry a verifiable end marker."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT=Path(__file__).resolve().parents[1]

class VoiceWifiFramingTest(unittest.TestCase):
    def test_short_writes_frame_and_checksum(self):
        source=(ROOT/'main/pet_p4_voice_wifi.c').read_text()
        marker='static bool voice_wifi_send_all('
        if marker not in source: marker='bool pet_p4_voice_wifi_active('
        functions=marker+source.rsplit(marker,1)[1].split('\n/* 按 NVS',1)[0]
        harness=r'''
#include <assert.h>
#include <errno.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>
#define ESP_OK 0
#define ESP_FAIL 1
#define ESP_ERR_INVALID_STATE 2
#define ESP_LOGW(...) ((void)0)
#define ESP_LOGI(...) ((void)0)
#define TAG "test"
#define SHUT_RDWR 2
typedef int esp_err_t;
static int s_sock=7;
static uint32_t s_total, s_checksum=0x811c9dc5u;
static uint8_t wire[64];static size_t used;static bool fail_send;
static int fake_send(int fd,const void *buf,size_t n,int flags){(void)fd;(void)flags;if(fail_send){errno=EIO;return -1;}if(n>2)n=2;memcpy(wire+used,buf,n);used+=n;return (int)n;}
static int fake_shutdown(int fd,int how){(void)fd;(void)how;return 0;}
static int fake_close(int fd){(void)fd;return 0;}
#define send fake_send
#define shutdown fake_shutdown
#define close fake_close
void pet_p4_voice_wifi_stop(void);
void pet_p4_voice_wifi_abort(void);
''' + functions + r'''
int main(void){uint8_t pcm[]={1,2,3,4};
  assert(pet_p4_voice_wifi_push(pcm,sizeof(pcm))==ESP_OK);
  pet_p4_voice_wifi_stop();
  assert(used==16 && wire[0]==4 && wire[1]==0 && memcmp(wire+2,pcm,4)==0);
  assert(wire[6]==0 && wire[7]==0 && wire[8]==4 && wire[9]==0 && wire[10]==0 && wire[11]==0);
  uint32_t hash=0x811c9dc5u;for(size_t i=0;i<4;i++)hash=(hash^pcm[i])*0x01000193u;
  assert(wire[12]==(uint8_t)hash && wire[13]==(uint8_t)(hash>>8) && wire[14]==(uint8_t)(hash>>16) && wire[15]==(uint8_t)(hash>>24));
  s_sock=7;used=0;fail_send=true;assert(pet_p4_voice_wifi_push(pcm,4)==ESP_FAIL);assert(s_sock==-1 && used==0);
}
'''
        with tempfile.TemporaryDirectory() as tmp:
            path=Path(tmp)/'test.c';binary=Path(tmp)/'test';path.write_text(harness)
            subprocess.run(['cc','-std=c11','-Wall','-Wextra','-Werror','-Wno-unused-function','-Wno-unused-variable',str(path),'-o',str(binary)],check=True)
            subprocess.run([str(binary)],check=True,timeout=15)
if __name__=='__main__':unittest.main()
