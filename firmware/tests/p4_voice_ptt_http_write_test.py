"""Complete short HTTP writes; fail on zero or negative progress."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT=Path(__file__).resolve().parents[1]

class VoicePttHttpWriteTest(unittest.TestCase):
    def test_partial_and_stalled_writes(self):
        source=(ROOT/'main/pet_p4_voice_ptt.c').read_text()
        marker='static bool ptt_http_write_all('
        function=marker+source.split(marker,1)[1].split('\nstatic void ptt_worker_task',1)[0] if marker in source else 'static bool ptt_http_write_all(esp_http_client_handle_t c,const char *b,size_t n){(void)c;(void)b;(void)n;return false;}'
        harness=r'''
#include <assert.h>
#include <stdbool.h>
#include <stddef.h>
#include <string.h>
typedef void *esp_http_client_handle_t;
static int limit, calls, written;
static int esp_http_client_write(esp_http_client_handle_t c,const char *buf,int n) {
  (void)c;(void)buf;calls++;if(limit<=0)return limit;
  int k=n<limit?n:limit;written+=k;return k;
}
''' + function + r'''
int main(void){char data[10]={0};limit=3;calls=written=0;
  assert(ptt_http_write_all((void*)1,data,sizeof(data)));assert(calls==4&&written==10);
  limit=0;calls=written=0;assert(!ptt_http_write_all((void*)1,data,sizeof(data)));assert(calls==1);
  limit=-1;calls=written=0;assert(!ptt_http_write_all((void*)1,data,sizeof(data)));assert(calls==1);
}
'''
        with tempfile.TemporaryDirectory() as tmp:
            path=Path(tmp)/'test.c';binary=Path(tmp)/'test';path.write_text(harness)
            subprocess.run(['cc','-std=c11','-Wall','-Wextra','-Werror','-Wno-unused-function',str(path),'-o',str(binary)],check=True)
            subprocess.run([str(binary)],check=True,timeout=15)

if __name__=='__main__':unittest.main()
