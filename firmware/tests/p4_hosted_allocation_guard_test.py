"""Exercise the pinned ESP-Hosted TX functions when their buffer allocation fails."""
import importlib.util
from pathlib import Path
import subprocess

import pytest
from p4_host_surface_snapshot_test import function

ROOT = Path(__file__).resolve().parents[1]
UPSTREAM = ROOT / 'managed_components/espressif__esp_hosted/host/drivers/transport/transport_drv.c'
SPEC = importlib.util.spec_from_file_location('hosted_patch', ROOT / 'tools/patch_hosted_transport.py')
PATCH = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PATCH)


def test_unrecognized_upstream_is_rejected():
    with pytest.raises(ValueError, match='unexpected'):
        PATCH.patch('different version')


def test_actual_tx_functions_return_error_without_copy_when_memory_is_exhausted(tmp_path):
    upstream = UPSTREAM.read_text()
    patched = PATCH.patch(upstream)
    funcs = '\n'.join(function(patched, signature) for signature in (
        'static esp_err_t transport_drv_sta_tx(void *h, void *buffer, size_t len)',
        'static esp_err_t transport_drv_ap_tx(void *h, void *buffer, size_t len)'))
    code = r'''
#include <assert.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <errno.h>
#include <string.h>
typedef int esp_err_t;
#define ESP_OK 0
#define ESP_ERR_ESP_NETIF_TX_FAILED 99
#define ESP_ERR_ESP_NETIF_NO_MEM 98
#define ESP_STA_IF 0
#define ESP_AP_IF 1
#define MAX_TRANSPORT_BUFFER_SIZE 256
#define H_ESP_PAYLOAD_HEADER_OFFSET 4
#define H_BUFF_ZEROCOPY 1
#define unlikely(x) (x)
struct mempool { int unused; };
struct channel { void *api_chan; void *memp; };
static struct channel a, b;
static struct channel *chan_arr[] = { &a, &b };
static bool wifi_tx_throttling;
static unsigned char buf[256];
static int exhausted, copied, transmitted;
static void *mempool_alloc(struct mempool *p, int size, bool zero) {
    (void)p; (void)size; (void)zero; return exhausted ? NULL : buf;
}
static void *copy(void *dst,const void *src,size_t n){copied++;return memcpy(dst,src,n);}
static struct { void *(*_h_memcpy)(void *,const void *,size_t); } functions = { copy };
static struct { typeof(functions) *funcs; } g_h = { &functions };
static void transport_sta_free_cb(void *p) {(void)p;}
static void transport_ap_free_cb(void *p) {(void)p;}
static int esp_hosted_tx(int a,int b,void *c,size_t d,int e,void (*f)(void *)) {
    (void)a;(void)b;(void)c;(void)d;(void)e;(void)f;transmitted++;return ESP_OK;
}
''' + funcs + r'''
int main(void) {
    char data[] = "abc"; a.api_chan=&a; b.api_chan=&b;
    exhausted=1;
    assert(transport_drv_sta_tx(&a,data,3)==ESP_ERR_ESP_NETIF_TX_FAILED);
    assert(transport_drv_ap_tx(&b,data,3)==ESP_ERR_ESP_NETIF_TX_FAILED);
    assert(copied==0 && transmitted==0);
    exhausted=0;
    assert(transport_drv_sta_tx(&a,data,3)==ESP_OK);
    assert(transport_drv_ap_tx(&b,data,3)==ESP_OK);
    assert(copied==2 && transmitted==2 && !memcmp(buf+4,data,3));
}
'''
    source = tmp_path / 'hosted_guard.c'
    source.write_text(code)
    binary = tmp_path / 'hosted_guard'
    subprocess.run(['cc', '-std=gnu11', '-Wall', '-Wextra', '-Werror', str(source), '-o', str(binary)], check=True)
    subprocess.run([str(binary)], check=True)
