"""Run the actual streaming RX parser with allocation failures and bad lengths."""
import importlib.util
from pathlib import Path
import subprocess

import pytest
from p4_host_surface_snapshot_test import function

ROOT = Path(__file__).resolve().parents[1]
UPSTREAM = ROOT / 'managed_components/espressif__esp_hosted/host/drivers/transport/sdio/sdio_drv.c'
SPEC = importlib.util.spec_from_file_location('sdio_patch', ROOT / 'tools/patch_hosted_sdio.py')
PATCH = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PATCH)


def test_unknown_sdio_revision_fails_closed():
    with pytest.raises(ValueError, match='unexpected'):
        PATCH.patch('other source')


def test_stream_rx_handles_exhaustion_and_rejects_invalid_length_before_allocating(tmp_path):
    source = PATCH.patch(UPSTREAM.read_text())
    marker = '// extract packets from the stream and push on to the queue'
    fn = function(source[source.index(marker):], 'static esp_err_t sdio_push_data_to_queue(')
    harness = r'''
#include <assert.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>
typedef int esp_err_t;
#define ESP_OK 0
#define ESP_FAIL -1
#define ESP_ERR_NO_MEM 99
#define ESP_LOGE(...) ((void)0)
#define ESP_LOGI(...) ((void)0)
#define MEMSET_REQUIRED 1
static int allocs, pushes, exhausted, malformed;
static uint8_t pkt[128];
static bool is_valid_sdio_rx_packet(uint8_t *p, uint16_t *len, uint16_t *offset) {
    (void)p; *len=malformed?20:4; *offset=2; return true;
}
static uint8_t *sdio_buffer_alloc(int flags) {(void)flags;allocs++;return exhausted?NULL:pkt;}
static int sdio_push_pkt_to_queue(uint8_t *p,uint16_t len,uint16_t off) {
    assert(p==pkt && len==4 && off==2); pushes++;return 0;
}
''' + fn + r'''
int main(void) {
    uint8_t input[6]={1,2,3,4,5,6};
    malformed=1;assert(sdio_push_data_to_queue(input,sizeof(input))==ESP_FAIL);
    assert(allocs==0 && pushes==0);
    malformed=0;exhausted=1;
    assert(sdio_push_data_to_queue(input,sizeof(input))==ESP_ERR_NO_MEM);
    assert(allocs==1 && pushes==0);
    exhausted=0;assert(sdio_push_data_to_queue(input,sizeof(input))==ESP_OK);
    assert(allocs==2 && pushes==1 && !memcmp(pkt,input,sizeof(input)));
}
'''
    path = tmp_path / 'sdio_guard.c'
    path.write_text(harness)
    exe = tmp_path / 'sdio_guard'
    subprocess.run(['cc', '-std=c11', '-Wall', '-Wextra', '-Werror', str(path), '-o', str(exe)], check=True)
    subprocess.run([str(exe)], check=True)
