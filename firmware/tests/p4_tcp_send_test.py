"""A closed or stalled socket must not spin forever while sending a line."""
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]

class TcpSendTest(unittest.TestCase):
    def test_zero_send_is_failure_and_temporary_backpressure_completes_within_budget(self):
        source = (ROOT / 'main/pet_p4_tcp_bsp.c').read_text()
        budget = re.search(r'^#define TCP_SEND_RETRY_BUDGET_US (\d+)$', source, re.MULTILINE)
        self.assertIsNotNone(budget, 'TCP send retry budget must be explicit and testable')
        function = 'static int tcp_send_all(' + source.split(
            'static int tcp_send_all(', 1)[1].split(
            '\n/* All real transport-route objects', 1)[0]
        harness = r'''
#include <assert.h>
#include <errno.h>
#include <stddef.h>
#include <stdint.h>
#define MSG_DONTWAIT 8
#define pdMS_TO_TICKS(ms) (ms)
#define TCP_SEND_RETRY_BUDGET_US ''' + budget.group(1) + r'''
static int mode, calls, last_flags, delays;
static int64_t now_us;
int64_t esp_timer_get_time(void) { return now_us; }
void vTaskDelay(int ticks) { delays += ticks; now_us += 1000; }
static int fake_send(int fd, const void *buf, size_t len, int flags) {
  (void)fd; (void)buf; last_flags=flags; calls++;
  if (mode == 0) { if (calls > 1) {errno = EIO; return -1;} return 0; }
  if (mode == 2) { if (calls == 1) {errno = EAGAIN; return -1;} return len > 2 ? 2 : (int)len; }
  if (mode == 3) { errno = EAGAIN; return -1; }
  if (mode == 4) { if (calls <= 61) { errno = EAGAIN; return -1; } return (int)len; }
  if (mode == 5) { if (calls <= 750) { errno = EAGAIN; return -1; } return (int)len; }
  if (calls == 1) {errno = EINTR; return -1;}
  return len > 2 ? 2 : (int)len;
}
#define send fake_send
''' + function + r'''
int main(void) {
  mode=0; calls=0; assert(tcp_send_all(1,"abc",3)==-1); assert(calls==1);
  mode=1; calls=0; last_flags=0; assert(tcp_send_all(1,"abcde",5)==0); assert(calls==4);
  assert(last_flags==MSG_DONTWAIT);
  mode=2; calls=0; delays=0; now_us=0; assert(tcp_send_all(1,"abcde",5)==0);
  assert(delays==1 && calls==4);
  mode=3; calls=0; delays=0; now_us=0; assert(tcp_send_all(1,"abc",3)==-1);
  assert(delays > 0 && delays <= TCP_SEND_RETRY_BUDGET_US / 1000
    && calls <= TCP_SEND_RETRY_BUDGET_US / 1000 + 1);
  // Wi-Fi may need several delayed-ACK intervals before its initial TCP
  // window drains. This must remain a bounded retry, not be mistaken for a
  // disconnected client after the old 20ms budget.
  mode=4; calls=0; delays=0; now_us=0; assert(tcp_send_all(1,"hello",5)==0);
  assert(delays==61 && calls==62);
  // Large configuration/inventory replies need more than one Wi-Fi delayed
  // ACK interval. They must retain their JSONL tail instead of disconnecting
  // after the first send window (about 5.7KB on this board).
  mode=5; calls=0; delays=0; now_us=0; assert(tcp_send_all(1,"config",6)==0);
  assert(delays==750 && calls==751);
  return 0;
}
'''
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'test.c'
            binary = Path(tmp) / 'test'
            path.write_text(harness)
            subprocess.run(['cc', '-std=c11', '-Wall', '-Wextra', '-Werror',
                            str(path), '-o', str(binary)], check=True)
            subprocess.run([str(binary)], check=True, timeout=15)

if __name__ == '__main__':
    unittest.main()
