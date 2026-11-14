"""A reply from a retired TCP connection cannot reach its successor."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]

class TcpSessionIdentityTest(unittest.TestCase):
    def test_reused_descriptor_does_not_retarget_queued_reply(self):
        source = (ROOT / 'main/pet_p4_tcp_bsp.c').read_text()
        sending = 'static int tcp_send_all(' + source.split('static int tcp_send_all(',1)[1].split('\nbool pet_p4_tcp_client_connected',1)[0]
        session = 'static void tcp_client_session(int fd) {' + source.split('static void tcp_client_session(int fd) {',1)[1].split('\nvoid pet_p4_tcp_set_route(',1)[0]
        harness = r'''
#include <assert.h>
#include <errno.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#define TCP_LINE_MAX (30 * 1024)
#define MALLOC_CAP_SPIRAM 0
#define MALLOC_CAP_8BIT 0
#define TAG "test"
#define ESP_LOGI(...) ((void)0)
#define ESP_LOGW(...) ((void)0)
#define portMAX_DELAY 0
#define SHUT_RDWR 2
#define MSG_DONTWAIT 8
#define pdMS_TO_TICKS(ms) (ms)
#define TCP_SEND_RETRY_BUDGET_US 20000
static int64_t test_now_us;
int64_t esp_timer_get_time(void) { return test_now_us; }
void vTaskDelay(int ticks) { test_now_us += (int64_t)ticks * 1000; }
typedef void (*pet_p4_send_line_fn)(const char *,void *);
static atomic_int g_client_fd = -1;
static atomic_uint g_session_id;
static atomic_uintptr_t g_session_ctx;
static void *g_tx_mutex = (void *)1;
static int route; static const void *g_route_ctx=&route;
static void (*g_hello)(void (*)(const char *,void *),void *);
static int writes, fail_send, shutdowns;
static int fake_send(int fd,const void *buf,size_t len,int flags) {(void)fd;(void)buf;(void)flags; writes++;return fail_send ? -1 : (int)len;}
static int fake_shutdown(int fd,int how) {(void)fd;(void)how;shutdowns++;return 0;}
static int fake_close(int fd) {(void)fd;return 0;}
static int recv_step;
static int fake_recv(int fd,void *buf,size_t n,int flags) {(void)fd;(void)n;(void)flags;if(recv_step++)return 0;memcpy(buf,"x\n",2);return 2;}
#define send fake_send
#define shutdown fake_shutdown
#define close fake_close
#define recv fake_recv
static int xSemaphoreTake(void *m,int ticks){(void)m;(void)ticks;return 1;}
static void xSemaphoreGive(void *m){(void)m;}
static void *heap_caps_malloc(size_t n,int caps){(void)caps;return malloc(n);}
static void *contexts[2]; static int queued;
static bool enqueue(const char *s,void (*send_fn)(const char *,void *),void *ctx,void *u){(void)s;(void)send_fn;(void)u;contexts[queued++]=ctx;return true;}
static bool (*g_enqueue)(const char *,void (*)(const char *,void *),void *,void *)=enqueue;
''' + sending + session + r'''
int main(void) {
  recv_step=0; tcp_client_session(42);
  recv_step=0; tcp_client_session(42);
  assert(queued==2);
  assert(pet_p4_tcp_context_is_tcp(contexts[0]));
  assert(pet_p4_tcp_context_is_tcp(contexts[1]));
  assert(((uintptr_t)contexts[0] & 1U) == 1U);
  assert(((uintptr_t)contexts[1] & 1U) == 1U);
  atomic_store(&g_client_fd,42);
  assert(!pet_p4_tcp_context_is_current(contexts[0]));
  assert(pet_p4_tcp_context_is_current(contexts[1]));
  assert(!pet_p4_tcp_request_current(tcp_send_line,contexts[0]));
  assert(pet_p4_tcp_request_current(tcp_send_line,contexts[1]));
  writes=0; tcp_send_line("old",contexts[0]); assert(writes==0);
  tcp_send_line("current",contexts[1]); assert(writes==2);
  fail_send=1; tcp_send_line("failure",contexts[1]);
  assert(atomic_load(&g_client_fd)==-1 && shutdowns==1);
  assert(!pet_p4_tcp_request_current(tcp_send_line,contexts[1]));
}
'''
        with tempfile.TemporaryDirectory() as tmp:
            path=Path(tmp)/'test.c'; binary=Path(tmp)/'test';path.write_text(harness)
            subprocess.run(['cc','-std=c11','-Wall','-Wextra','-Werror','-Wno-unused-function','-Wno-unused-variable',
                            '-fsanitize=address,undefined',str(path),'-o',str(binary)],check=True)
            subprocess.run([str(binary)],check=True,timeout=15)

    def test_tcp_session_context_is_routed_without_pointer_dereference(self):
        tcp = (ROOT / 'main/pet_p4_tcp_bsp.c').read_text()
        main = (ROOT / 'main/pet_p4_main.c').read_text()
        self.assertIn('bool pet_p4_tcp_context_is_tcp(void *ctx)', tcp)
        self.assertIn('bool pet_p4_tcp_context_is_current(void *ctx)', tcp)
        self.assertIn('pet_p4_tcp_context_is_tcp(ctx)', main)
        self.assertIn('route == PET_P4_TRANSPORT_TCP', main)
        self.assertIn('pet_p4_tcp_context_is_current(ctx)', main)
        route_start = main.index('static void transport_send_line_impl(')
        route_end = main.index('\nstatic void transport_send_line(', route_start)
        route_body = main[route_start:route_end]
        self.assertIn('ctx && !tcp_route', route_body)
        self.assertIn('pet_p4_tcp_broadcast_line(line);', route_body)

if __name__ == '__main__': unittest.main()
