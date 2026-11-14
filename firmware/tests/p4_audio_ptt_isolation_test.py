"""Local Wi-Fi PTT must not start an empty desktop PCM session."""
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def test_local_ptt_does_not_publish_pcm_begin_or_end(tmp_path):
    source = (ROOT / 'main/pet_p4_audio.c').read_text()
    begin = 'static void send_audio_begin(' + source.split('static void send_audio_begin(', 1)[1].split('\nstatic bool send_audio_chunk(', 1)[0]
    end = 'static void send_audio_end(' + source.split('static void send_audio_end(', 1)[1].split('\nstatic uint16_t read_le16(', 1)[0]
    code = r'''
#include <assert.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#define PET_P4_AUDIO_SAMPLE_RATE 16000
#define PET_P4_AUDIO_CHANNELS 1
#define PET_P4_AUDIO_BITS_PER_SAMPLE 16
#define PET_P4_AUDIO_FRAME_MS 20
typedef int cJSON;
static char g_board_device_id[]="board";
static bool capturing;
static int begin_count,end_count,complete_count,abort_count;
bool pet_p4_voice_ptt_capturing(void) {return capturing;}
void pet_p4_voice_ptt_notify_end(void) {if(capturing){complete_count++;capturing=false;}}
void pet_p4_voice_ptt_abort(void) {if(capturing){abort_count++;capturing=false;}}
static void pet_p4_audio_diagnostic(const char *a,const char *b,int c,bool d) {(void)a;(void)b;(void)c;(void)d;}
static bool pet_p4_audio_conversation_active(void) {return false;}
static void add_conversation_metadata(cJSON *o) {(void)o;}
static cJSON *cJSON_CreateObject(void) {static int obj;return &obj;}
static void cJSON_AddStringToObject(cJSON *o,const char *k,const char *v) {(void)o;(void)k;(void)v;}
static void cJSON_AddNumberToObject(cJSON *o,const char *k,double v) {(void)o;(void)k;(void)v;}
static void cJSON_AddBoolToObject(cJSON *o,const char *k,bool v) {(void)o;(void)k;(void)v;}
static void send_topic(const char *t,cJSON *o) {(void)o;if(!strcmp(t,"audio/begin"))begin_count++;if(!strcmp(t,"audio/end"))end_count++;}
''' + begin + '\n' + end + r'''
int main(void) {
 capturing=true;send_audio_begin("local",true,"ptt-local");send_audio_end("local","released",1,320,0);
 assert(begin_count==0 && end_count==0 && complete_count==1);
 capturing=true;send_audio_begin("failed",true,"ptt-local");send_audio_end("failed","read_error",1,320,0);
 assert(begin_count==0 && end_count==0 && abort_count==1);
 send_audio_begin("normal",true,"");send_audio_end("normal","released",1,320,0);
 assert(begin_count==1 && end_count==1);
}
'''
    path=tmp_path/'ptt.c';path.write_text(code);exe=tmp_path/'ptt'
    subprocess.run(['cc','-std=c11','-Wall','-Wextra','-Werror',str(path),'-o',str(exe)],check=True)
    subprocess.run([str(exe)],check=True)
