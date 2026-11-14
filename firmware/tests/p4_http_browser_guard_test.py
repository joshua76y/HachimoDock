"""Browser origins and simple form posts cannot inject device commands."""
from pathlib import Path
import subprocess
import tempfile
import unittest
ROOT=Path(__file__).resolve().parents[1]

class HttpBrowserGuardTest(unittest.TestCase):
    def test_state_post_requires_origin_free_json(self):
        source=(ROOT/'main/pet_p4_http_bsp.c').read_text()
        marker='static bool http_request_is_non_browser_json('
        helper=marker+source.split(marker,1)[1].split('\nstatic esp_err_t state_post(',1)[0] if marker in source else ''
        handler='static esp_err_t state_post('+source.split('static esp_err_t state_post(',1)[1].split('\nstatic esp_err_t options_handler(',1)[0]
        harness=r'''
#include <assert.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdio.h>
#include <string.h>
#include <strings.h>
typedef int esp_err_t;
#define ESP_OK 0
typedef struct {const char *origin; const char *content_type; const char *status; int submitted;} httpd_req_t;
static size_t httpd_req_get_hdr_value_len(httpd_req_t *r,const char *name) {
  const char *v=!strcasecmp(name,"Origin")?r->origin:!strcasecmp(name,"Content-Type")?r->content_type:NULL;return v?strlen(v):0;
}
static int httpd_req_get_hdr_value_str(httpd_req_t *r,const char *name,char *out,size_t len) {
  const char *v=!strcasecmp(name,"Origin")?r->origin:!strcasecmp(name,"Content-Type")?r->content_type:NULL;
  if(!v || strlen(v)>=len)return 1;strcpy(out,v);return ESP_OK;
}
static bool submit_body(httpd_req_t *r) {r->submitted++;return true;}
static int json_reply(httpd_req_t *r,const char *s,const char *b) {(void)b;r->status=s;return ESP_OK;}
''' + helper + handler + r'''
int main(void){httpd_req_t r={.origin="https://evil.example",.content_type="application/json"};
  state_post(&r);assert(r.submitted==0 && r.status && !strcmp(r.status,"403 Forbidden"));
  r=(httpd_req_t){.content_type="text/plain"};state_post(&r);assert(r.submitted==0);
  r=(httpd_req_t){.content_type="application/jsonx"};state_post(&r);assert(r.submitted==0);
  r=(httpd_req_t){.content_type="application/json"};state_post(&r);assert(r.submitted==1);
  r=(httpd_req_t){.content_type="application/json; charset=utf-8"};state_post(&r);assert(r.submitted==1);
}
'''
        with tempfile.TemporaryDirectory() as tmp:
            path=Path(tmp)/'test.c';binary=Path(tmp)/'test';path.write_text(harness)
            subprocess.run(['cc','-std=c11','-Wall','-Wextra','-Werror','-Wno-unused-function','-Wno-misleading-indentation',str(path),'-o',str(binary)],check=True)
            subprocess.run([str(binary)],check=True,timeout=15)
if __name__=='__main__':unittest.main()
