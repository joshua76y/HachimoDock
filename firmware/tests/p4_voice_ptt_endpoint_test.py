"""Resolve the PTT target without a machine-specific hard-coded address."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT=Path(__file__).resolve().parents[1]

class VoicePttEndpointTest(unittest.TestCase):
    def test_explicit_peer_and_missing_endpoints(self):
        source=(ROOT/'main/pet_p4_voice_ptt.c').read_text()
        marker='static bool ptt_resolve_endpoint('
        function=marker+source.split(marker,1)[1].split('\nstatic bool ptt_make_url',1)[0] if marker in source else 'static bool ptt_resolve_endpoint(const char *c,const char *p,char *o,size_t n){(void)c;(void)p;(void)o;(void)n;return false;}'
        harness=r'''
#include <assert.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdio.h>
#include <string.h>
''' + function + r'''
int main(void) {
  char out[128];
  assert(ptt_resolve_endpoint("https://host/custom", "192.0.2.7", out, sizeof(out)));
  assert(strcmp(out,"https://host/custom")==0);
  assert(ptt_resolve_endpoint("", "192.0.2.7", out, sizeof(out)));
  assert(strcmp(out,"http://192.0.2.7:8901/voice")==0);
  assert(!ptt_resolve_endpoint("", "", out, sizeof(out)));
  assert(!ptt_resolve_endpoint("", "192.0.2.7", out, 8));
}
'''
        with tempfile.TemporaryDirectory() as tmp:
            path=Path(tmp)/'test.c'; binary=Path(tmp)/'test';path.write_text(harness)
            subprocess.run(['cc','-std=c11','-Wall','-Wextra','-Werror',str(path),'-o',str(binary)],check=True)
            subprocess.run([str(binary)],check=True,timeout=15)
if __name__=='__main__':unittest.main()
