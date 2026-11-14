"""A normal recording must reach the session-aware audio protocol."""
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def test_normal_audio_uses_protocol_even_when_wifi_side_channel_is_available(tmp_path):
    source = (ROOT / 'main/pet_p4_audio.c').read_text()
    fn = 'static bool send_audio_chunk(' + source.split('static bool send_audio_chunk(', 1)[1].split('\nstatic void send_audio_end(', 1)[0]
    code = r'''
#include <assert.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#define PET_P4_AUDIO_FRAME_BYTES 640
#define ESP_OK 0
typedef int cJSON;
static bool local_ptt;
static int feed_count, topics, side_channel;
bool pet_p4_voice_ptt_capturing(void) {return local_ptt;}
void pet_p4_voice_ptt_feed(const uint8_t *p,size_t n) {(void)p;(void)n;feed_count++;}
bool pet_p4_voice_wifi_active(void) {return true;}
int pet_p4_voice_wifi_start_configured(void) {side_channel++;return 0;}
int pet_p4_voice_wifi_push(const void *p,size_t n) {(void)p;(void)n;side_channel++;return 0;}
static uint64_t fnv1a64_update(uint64_t h,const uint8_t *p,size_t n) {(void)p;(void)n;return h;}
static int mbedtls_base64_encode(unsigned char *out,size_t cap,size_t *used,const unsigned char *in,size_t len) {
    (void)in;(void)len;assert(cap>1);out[0]='A';*used=1;return 0;
}
static cJSON *cJSON_CreateObject(void) {static int result;return &result;}
static void cJSON_AddStringToObject(cJSON *o,const char *k,const char *v) {(void)o;(void)k;(void)v;}
static void cJSON_AddNumberToObject(cJSON *o,const char *k,double v) {(void)o;(void)k;(void)v;}
static void cJSON_AddBoolToObject(cJSON *o,const char *k,bool v) {(void)o;(void)k;(void)v;}
static void send_topic(const char *topic,cJSON *o) {(void)o;assert(!strcmp(topic,"audio/chunk"));topics++;}
static bool s_wifi_tried;
''' + fn + r'''
int main(void) {
    uint8_t audio[4]={0};
    assert(send_audio_chunk("s",0,audio,sizeof(audio),false,false,""));
    assert(topics==1 && side_channel==0 && feed_count==0);
    local_ptt=true;
    assert(send_audio_chunk("s",1,audio,sizeof(audio),false,false,""));
    assert(topics==1 && side_channel==0 && feed_count==1);
}
'''
    c_file = tmp_path / 'audio.c'
    c_file.write_text(code)
    exe = tmp_path / 'audio'
    subprocess.run(['cc','-std=c11','-Wall','-Wextra','-Werror','-Wno-unused-variable',str(c_file),'-o',str(exe)],check=True)
    subprocess.run([str(exe)],check=True)
