"""Wi-Fi disconnect handling must not sleep on the global event loop."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT=Path(__file__).resolve().parents[1]

class WifiReconnectTaskTest(unittest.TestCase):
    def test_disconnect_retries_outside_event_loop_and_coalesces(self):
        source=(ROOT/'main/pet_p4_wifi_bsp.c').read_text()
        marker='static void wifi_reconnect_task('
        helpers=marker+source.split(marker,1)[1].split('\nstatic void on_wifi_event(',1)[0] if marker in source else ''
        event='static void on_wifi_event('+source.split('static void on_wifi_event(',1)[1].split('\nesp_err_t pet_p4_wifi_init(',1)[0]
        harness=r'''
#include <assert.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdatomic.h>
#include <stdio.h>
#define TAG "test"
#define ESP_LOGW(...) ((void)0)
#define ESP_LOGI(...) ((void)0)
#define WIFI_EVENT 1
#define IP_EVENT 2
#define WIFI_EVENT_STA_START 1
#define WIFI_EVENT_STA_DISCONNECTED 2
#define IP_EVENT_STA_GOT_IP 3
#define PET_P4_WIFI_HOSTNAME "test"
#define IPSTR "%d"
#define IP2STR(x) 1
#define pdMS_TO_TICKS(x) (x)
#define pdPASS 1
typedef int esp_event_base_t;
typedef struct {struct {int ip;} ip_info;} ip_event_got_ip_t;
static bool s_connected; static char s_ip[32];
static atomic_bool s_reconnect_pending;
static unsigned delays, connects, created; static void (*task)(void *);
static void vTaskDelay(int n){(void)n;delays++;}
static int esp_wifi_connect(void){connects++;return 0;}
static void vTaskDelete(void *p){(void)p;}
static int xTaskCreate(void (*fn)(void *),const char *name,int stack,void *arg,int prio,void *handle){(void)name;(void)stack;(void)arg;(void)prio;(void)handle;task=fn;created++;return pdPASS;}
static bool pet_p4_wifi_is_connected(void){return s_connected;}
static void wifi_set_status(bool connected,const char *ip){(void)ip;s_connected=connected;}
static uint8_t wifi_failcnt_load(void){return 0;}
static void wifi_failcnt_save(uint8_t n){(void)n;}
static void sntp_start_once(void){}
''' + helpers + event + r'''
int main(void){
  s_connected=true;
  on_wifi_event(NULL,WIFI_EVENT,WIFI_EVENT_STA_DISCONNECTED,NULL);
  assert(delays==0 && connects==0 && created==1 && !s_connected);
  on_wifi_event(NULL,WIFI_EVENT,WIFI_EVENT_STA_DISCONNECTED,NULL);
  assert(created==1);
  assert(task); task(NULL); assert(delays==1 && connects==1);
  on_wifi_event(NULL,WIFI_EVENT,WIFI_EVENT_STA_DISCONNECTED,NULL);
  assert(created==2); s_connected=true; task(NULL);
  assert(delays==2 && connects==1);
}
'''
        with tempfile.TemporaryDirectory() as tmp:
            path=Path(tmp)/'test.c';binary=Path(tmp)/'test';path.write_text(harness)
            subprocess.run(['cc','-std=c11','-Wall','-Wextra','-Werror','-Wno-unused-function','-Wno-unused-variable',str(path),'-o',str(binary)],check=True)
            subprocess.run([str(binary)],check=True,timeout=15)
if __name__=='__main__':unittest.main()
