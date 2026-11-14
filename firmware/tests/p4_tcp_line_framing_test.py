"""Exercise the real TCP receive loop with fragmented and overlong input."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]

class TcpLineFramingTest(unittest.TestCase):
    def test_overlong_line_is_discarded_until_newline(self):
        source = (ROOT / 'main/pet_p4_tcp_bsp.c').read_text()
        session = 'static void tcp_client_session(int fd) {' + source.split(
            'static void tcp_client_session(int fd) {', 1)[1].split(
            '\nvoid pet_p4_tcp_set_route(', 1)[0]
        harness = r'''
#include <assert.h>
#include <stdbool.h>
#include <stdint.h>

#include <stdatomic.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
#define TCP_LINE_MAX (30 * 1024)
#define MALLOC_CAP_SPIRAM 0
#define MALLOC_CAP_8BIT 0
#define TAG "test"
#define ESP_LOGI(...) ((void)0)
#define portMAX_DELAY 0
#define TCP_SESSION_CONTEXT_TAG ((uintptr_t)1U)
static const char *input;
static size_t input_len, input_pos, chunk_limit;
static int count;
static char last[TCP_LINE_MAX + 1];
static atomic_int g_client_fd = -1;
static atomic_uint g_session_id;
static atomic_uintptr_t g_session_ctx;
static void *g_tx_mutex = (void *)1;
static int xSemaphoreTake(void *m,int ticks) {(void)m;(void)ticks;return 1;}
static void xSemaphoreGive(void *m) {(void)m;}
static void (*g_hello)(void (*)(const char *, void *), void *);
static void tcp_send_line(const char *s, void *ctx) {(void)s; (void)ctx;}
static void *heap_caps_malloc(size_t n, int caps) {(void)caps; return malloc(n);}
static int fake_recv(int fd, void *buf, size_t n, int flags) {
  (void)fd; (void)flags;
  if (input_pos == input_len) return 0;
  if (n > chunk_limit) n = chunk_limit;
  if (n > input_len - input_pos) n = input_len - input_pos;
  memcpy(buf, input + input_pos, n); input_pos += n; return (int)n;
}
static int fake_close(int fd) {(void)fd; return 0;}
#define recv fake_recv
#define close fake_close
static int enqueue(const char *s, void (*send)(const char *, void *), void *ctx, void *unused) {
  (void)send; (void)ctx; (void)unused; count++; strcpy(last, s); return 1;
}
static int (*g_enqueue)(const char *, void (*)(const char *, void *), void *, void *) = enqueue;
''' + session + r'''
static void run(size_t extra, size_t chunk) {
  const char *valid = "{\"topic\":\"hello\"}";
  size_t n = TCP_LINE_MAX + extra;
  char *stream = malloc(n + 1 + strlen(valid) + 1);
  assert(stream);
  memset(stream, 'x', n); stream[n] = '\n';
  memcpy(stream + n + 1, valid, strlen(valid) + 1);
  stream[n + 1 + strlen(valid)] = '\n';
  input = stream; input_len = n + 2 + strlen(valid); input_pos = 0;
  chunk_limit = chunk; count = 0; last[0] = 0;
  tcp_client_session(1);
  assert(count == (extra ? 1 : 2));
  assert(strcmp(last, valid) == 0);
  free(stream);
}
static void run_nul(void) {
  static const char stream[] = "{\"topic\":\"bad\"}\0ignored\n{\"topic\":\"hello\"}\n";
  input = stream; input_len = sizeof(stream) - 1; input_pos = 0;
  chunk_limit = 7; count = 0; last[0] = 0;
  tcp_client_session(1);
  assert(count == 1);
  assert(strcmp(last, "{\"topic\":\"hello\"}") == 0);
}
int main(void) {
  run(0, 2048); run(1, 2048); run(17, 2048); run(3000, 2048);
  run(1, 1); run(3000, 137);
  run_nul();
  return 0;
}
'''
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'test.c'
            binary = Path(tmp) / 'test'
            path.write_text(harness)
            subprocess.run(['cc', '-std=c11', '-Wall', '-Wextra', '-Werror',
                            '-fsanitize=address,undefined', str(path), '-o', str(binary)], check=True)
            subprocess.run([str(binary)], check=True)

if __name__ == '__main__':
    unittest.main()
