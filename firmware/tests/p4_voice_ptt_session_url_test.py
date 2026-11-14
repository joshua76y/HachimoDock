"""Both PTT HTTP requests must carry the same bounded session identifier."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT=Path(__file__).resolve().parents[1]

class VoicePttSessionUrlTest(unittest.TestCase):
    def test_session_url_preserves_query_and_rejects_invalid_id(self):
        source=(ROOT/'main/pet_p4_voice_ptt.c').read_text()
        marker='static bool ptt_make_url('
        function=marker+source.split(marker,1)[1].split('\nstatic char *ptt_begin_url_dup',1)[0] if marker in source else 'static bool ptt_make_url(const char *b,const char *i,bool s,char *o,size_t n){(void)b;(void)i;(void)s;(void)o;(void)n;return false;}'
        harness=r'''
#include <assert.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdio.h>
#include <string.h>
''' + function + r'''
int main(void){char out[192];const char *id="0123456789abcdef";
  assert(ptt_make_url("http://host/voice",id,false,out,sizeof(out)));
  assert(strcmp(out,"http://host/voice?sid=0123456789abcdef")==0);
  assert(ptt_make_url("http://host/voice?engine=a",id,true,out,sizeof(out)));
  assert(strcmp(out,"http://host/voice/begin?engine=a&sid=0123456789abcdef")==0);
  assert(!ptt_make_url("http://host/voice","short",false,out,sizeof(out)));
  assert(!ptt_make_url("http://host/voice#frag",id,false,out,sizeof(out)));
  assert(!ptt_make_url("http://host/voice",id,false,out,8));
}
'''
        with tempfile.TemporaryDirectory() as tmp:
            path=Path(tmp)/'test.c';binary=Path(tmp)/'test';path.write_text(harness)
            subprocess.run(['cc','-std=c11','-Wall','-Wextra','-Werror',str(path),'-o',str(binary)],check=True)
            subprocess.run([str(binary)],check=True,timeout=15)
if __name__=='__main__':unittest.main()
