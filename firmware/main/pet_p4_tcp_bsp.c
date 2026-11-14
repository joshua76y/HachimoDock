/*
 * 路线 A：USB JSON Lines 协议原样套 TCP —— 固件侧传输层。
 *
 * 与 http_bsp（路线 B）的本质区别：TCP 通道带真实 send_line，
 * 协议层的 ACK / 响应能原路返回 PC，恢复双向应答与 29 组件生态。
 *
 * 架构（镜像 uart/usb 传输任务的设计）：
 *   - 监听 0.0.0.0:PET_P4_TCP_PORT，单客户端（后来的踢掉前面的可选，先实现拒绝）
 *   - 读任务逐行收 JSON（上限 30KB，与 HTTP_MAX_LINE 对齐），
 *     每行经注入的 enqueue 函数进入同一个协议队列
 *   - 回程：协议任务调用 tcp_send_line(line, reply_ctx) 直接写回客户端 socket
 *   - 主动广播（状态压缩器等 BROADCAST 行）由 main.c 的
 *     transport_send_line_impl 调 pet_p4_tcp_broadcast_line() 转发
 *   - 连接建立时发 hello（由 main.c 注入 hello 提供函数）
 *
 * TCP replies use an opaque per-session generation as reply_ctx.  They are
 * not passed to the UART-only screenshot/debug writers that dereference it.
 */
#include "pet_p4_tcp_bsp.h"

#include <errno.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "esp_heap_caps.h"
#include "esp_log.h"
#include "esp_timer.h"
#include "pet_p4_voice_wifi.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "lwip/sockets.h"

#define TAG "pet-p4-tcp"

#ifndef PET_P4_TCP_PORT
#define PET_P4_TCP_PORT 3333
#endif
#define TCP_LINE_MAX (30 * 1024)
#define TCP_TASK_STACK (8 * 1024)
#define TCP_BACKLOG 1
/* ESP lwIP initially accepts only one TCP send window (about 4 KB on this
 * target).  A complete hello is larger and Wi-Fi delayed ACKs commonly take
 * tens of milliseconds, so a 20 ms non-blocking retry tears down an otherwise
 * healthy connection halfway through its first JSON line.  Large configuration
 * and inventory replies can need several Wi-Fi windows; retain their JSONL
 * tail for up to three seconds rather than corrupting the stream. */
#define TCP_SEND_RETRY_BUDGET_US 3000000

/* main.c 注入 */
static pet_p4_tcp_enqueue_fn g_enqueue;
static pet_p4_tcp_hello_fn g_hello;
static atomic_int g_client_fd = -1;  /* 当前客户端，-1 = 无 */
static atomic_uint g_session_id;    /* 断线重连不能复用前一会话的回复 */
static atomic_uintptr_t g_session_ctx;
static SemaphoreHandle_t g_tx_mutex;
static atomic_bool g_listening;
static atomic_bool g_listener_started;
#define PET_TCP_TASK_PRIORITY 5

static int tcp_send_all(int fd, const char *buf, size_t len) {
  size_t off = 0;
  const int64_t deadline_us = esp_timer_get_time() + TCP_SEND_RETRY_BUDGET_US;
  while (off < len) {
    /* Audio frames are emitted from the real-time capture path.  Never let a
     * congested LAN peer hold g_tx_mutex (and therefore capture) for the
     * socket's multi-second send timeout.  Non-blocking TCP may split a valid
     * line, though, so allow a short bounded retry window before dropping it. */
    int w = send(fd, buf + off, len - off, MSG_DONTWAIT);
    if (w < 0) {
      if (errno == EINTR) continue;
      if ((errno == EAGAIN || errno == EWOULDBLOCK)
          && esp_timer_get_time() < deadline_us) {
        vTaskDelay(pdMS_TO_TICKS(1));
        continue;
      }
      return -1;
    }
    if (w == 0) return -1;
    off += (size_t) w;
  }
  return 0;
}

/* All real transport-route objects have pointer alignment.  Reserve the low
 * bit for a TCP generation token so generic delayed producers (notably bubble
 * pixels) can recognize it without treating it as a route-object address. */
#define TCP_SESSION_CONTEXT_TAG ((uintptr_t) 1U)

bool pet_p4_tcp_context_is_tcp(void *ctx) {
  return ((uintptr_t) ctx & TCP_SESSION_CONTEXT_TAG) != 0;
}

bool pet_p4_tcp_context_is_current(void *ctx) {
  return pet_p4_tcp_context_is_tcp(ctx)
    && atomic_load(&g_client_fd) >= 0
    && (uintptr_t) ctx == atomic_load(&g_session_ctx);
}

/* 协议任务回程入口：写当前客户端（含 \n）。断线时静默关闭连接。 */
static void tcp_send_line(const char *line, void *ctx) {
  if (!line || !ctx || !g_tx_mutex) return;
  xSemaphoreTake(g_tx_mutex, portMAX_DELAY);
  int fd = atomic_load(&g_client_fd);
  if (fd < 0 || !pet_p4_tcp_context_is_current(ctx)) {
    xSemaphoreGive(g_tx_mutex);
    return;
  }
  size_t len = strlen(line);
  bool ok = tcp_send_all(fd, line, len) == 0 && tcp_send_all(fd, "\n", 1) == 0;
  if (!ok) {
    /* The session task alone closes fd; shutdown wakes its blocking recv. */
    atomic_store(&g_client_fd, -1);
    shutdown(fd, SHUT_RDWR);
    ESP_LOGW(TAG, "send failed, client dropped");
  }
  xSemaphoreGive(g_tx_mutex);
}

bool pet_p4_tcp_request_current(pet_p4_send_line_fn send_line, void *ctx) {
  if (send_line != tcp_send_line) return true;
  return pet_p4_tcp_context_is_current(ctx);
}

bool pet_p4_tcp_client_connected(void) {
  return atomic_load(&g_client_fd) >= 0;
}

void pet_p4_tcp_broadcast_line(const char *line) {
  if (!line || !g_tx_mutex) return;
  xSemaphoreTake(g_tx_mutex, portMAX_DELAY);
  int fd = atomic_load(&g_client_fd);
  if (fd < 0) {
    xSemaphoreGive(g_tx_mutex);
    return;
  }
  size_t len = strlen(line);
  bool ok = tcp_send_all(fd, line, len) == 0 && tcp_send_all(fd, "\n", 1) == 0;
  if (!ok) {
    atomic_store(&g_client_fd, -1);
    shutdown(fd, SHUT_RDWR);
  }
  xSemaphoreGive(g_tx_mutex);
}

static void tcp_client_session(int fd) {
  ESP_LOGI(TAG, "client connected fd=%d", fd);
  xSemaphoreTake(g_tx_mutex, portMAX_DELAY);
  unsigned session = atomic_fetch_add(&g_session_id, 1) + 1;
  if (!session) session = atomic_fetch_add(&g_session_id, 1) + 1;
  uintptr_t session_ctx_value = ((uintptr_t) session << 1) | TCP_SESSION_CONTEXT_TAG;
  atomic_store(&g_session_ctx, session_ctx_value);
  atomic_store(&g_client_fd, fd);
  xSemaphoreGive(g_tx_mutex);
  void *session_ctx = (void *) session_ctx_value;
  if (g_hello) g_hello(tcp_send_line, session_ctx);

  /* 行组装缓冲放 PSRAM，减内部堆压力 */
  char *acc = (char *) heap_caps_malloc(TCP_LINE_MAX + 1, MALLOC_CAP_SPIRAM | MALLOC_CAP_8BIT);
  size_t have = 0;
  bool discarding = false;
  char chunk[2048];
  if (acc) {
    while (true) {
      int r = recv(fd, chunk, sizeof(chunk), 0);
      if (r <= 0) break;
      for (int i = 0; i < r; i++) {
        char c = chunk[i];
        if (discarding) {
          if (c == '\n') discarding = false;
          continue;
        }
        if (c == '\0') {
          /* A C-string consumer would otherwise accept only a forged prefix. */
          have = 0;
          discarding = true;
          continue;
        }
        if (c == '\n') {
          acc[have] = '\0';
          /* 去掉可选 \r */
          if (have && acc[have - 1] == '\r') acc[have - 1] = '\0';
          if (have && g_enqueue) {
            g_enqueue(acc, tcp_send_line, session_ctx, NULL);
          }
          have = 0;
        } else if (have < TCP_LINE_MAX) {
          acc[have++] = c;
        } else {
          /* 丢弃整个超长行，直到换行，不能将其尾部当作新命令。 */
          have = 0;
          discarding = true;
        }
      }
    }
  }
  free(acc);
  xSemaphoreTake(g_tx_mutex, portMAX_DELAY);
  if (atomic_load(&g_client_fd) == fd) atomic_store(&g_client_fd, -1);
  close(fd);
  xSemaphoreGive(g_tx_mutex);
  ESP_LOGI(TAG, "client disconnected");
}

void pet_p4_tcp_set_route(const void *route_ctx) {
  (void) route_ctx; /* retained for callers; TCP uses a session generation */
}

void pet_p4_tcp_init(pet_p4_tcp_enqueue_fn enqueue, pet_p4_tcp_hello_fn hello) {
  if (atomic_exchange(&g_listening, true)) return;
  g_enqueue = enqueue;
  g_hello = hello;
  g_tx_mutex = xSemaphoreCreateMutex();
  if (!g_tx_mutex) return;
}

/* 监听任务：与 init 分离，由 main.c 在队列就绪后显式启动 */
static void tcp_listener_task(void *arg) {
  (void) arg;
  int listener = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
  if (listener < 0) {
    ESP_LOGE(TAG, "socket failed errno=%d", errno);
    atomic_store(&g_listener_started, false);
    vTaskDelete(NULL);
    return;
  }
  int reuse = 1;
  setsockopt(listener, SOL_SOCKET, SO_REUSEADDR, &reuse, sizeof(reuse));
  struct sockaddr_in addr = {
    .sin_family = AF_INET,
    .sin_addr.s_addr = htonl(INADDR_ANY),
    .sin_port = htons(PET_P4_TCP_PORT),
  };
  if (bind(listener, (struct sockaddr *) &addr, sizeof(addr)) < 0
      || listen(listener, TCP_BACKLOG) < 0) {
    ESP_LOGE(TAG, "bind/listen :%d failed errno=%d", PET_P4_TCP_PORT, errno);
    close(listener);
    atomic_store(&g_listener_started, false);
    vTaskDelete(NULL);
    return;
  }
  ESP_LOGI(TAG, "listening :%d (route A)", PET_P4_TCP_PORT);
  while (true) {
    struct sockaddr_in peer = {0};
    socklen_t peer_len = sizeof(peer);
    int fd = accept(listener, (struct sockaddr *) &peer, &peer_len);
    if (fd >= 0 && peer.sin_family == AF_INET) {
      char peer_ip[32];
      inet_ntoa_r(peer.sin_addr, peer_ip, sizeof(peer_ip));
      /* 记住 PC 地址：拔掉 USB 后 Wi-Fi 音频直接推给它 */
      pet_p4_voice_wifi_note_host(peer_ip);
      ESP_LOGI(TAG, "client %s connected", peer_ip);
    }
    if (fd < 0) {
      vTaskDelay(pdMS_TO_TICKS(200));
      continue;
    }
    /* keepalive，PC 端异常掉电也能及时释放。
     * 必须显式调短：lwip 默认 keepidle 2 小时，半开连接会一直占住
     * 单客户端位，新连接全部被拒（表现为 3333 全部超时，提示音/指令失灵）。 */
    int keep = 1;
    setsockopt(fd, SOL_SOCKET, SO_KEEPALIVE, &keep, sizeof(keep));
    int keepidle = 10, keepintvl = 5, keepcnt = 3;
    setsockopt(fd, IPPROTO_TCP, TCP_KEEPIDLE, &keepidle, sizeof(keepidle));
    setsockopt(fd, IPPROTO_TCP, TCP_KEEPINTVL, &keepintvl, sizeof(keepintvl));
    setsockopt(fd, IPPROTO_TCP, TCP_KEEPCNT, &keepcnt, sizeof(keepcnt));
    struct timeval tv = { .tv_sec = 30, .tv_usec = 0 };
    setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof(tv));
    /* 单客户端：已有连接则拒绝新连接 */
    int old = atomic_load(&g_client_fd);
    if (old >= 0) {
      ESP_LOGW(TAG, "second client rejected");
      close(fd);
      continue;
    }
    tcp_client_session(fd);
  }
}

void pet_p4_tcp_start_listener(void) {
  if (!g_tx_mutex || !g_enqueue) return;
  if (atomic_load(&g_listener_started)) return;
  atomic_store(&g_listener_started, true);
  if (xTaskCreate(tcp_listener_task, "pet-p4-tcp", TCP_TASK_STACK, NULL,
                  PET_TCP_TASK_PRIORITY, NULL) != pdPASS) {
    atomic_store(&g_listener_started, false);
    ESP_LOGW(TAG, "tcp listener task create failed");
  }
}
