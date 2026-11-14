/*
 * 路线 B：设备侧最小 HTTP server。
 *
 * 端点：
 *   GET  /health  -> {"ok":true}
 *   GET  /status  -> 设备 ID / 固件 / IP / uptime / 堆水位 / POST 计数
 *   POST /state   -> body 是一行 JSON Lines，转交 g_protocol_queue
 *   POST /line    -> /state 的别名，便于以后直接复用 USB 协议客户端
 *
 * 设计约束：
 *  - 响应体都很小。RLCD 踩过「一次 httpd_resp_send 15KB 把服务器堵死」的坑，
 *    这里不做大块推送；语音/截图等大数据走后面的路线 A。
 *  - POST 是 fire-and-forget：协议层的 ACK 由 main.c 提供的 send_line 丢弃，
 *    HTTP 只回入队结果。要等 ACK 属于路线 A 的 TCP 双向通道。
 *  - 不访问 g_state，避免跨任务读写运行时状态。
 */
#include "pet_p4_http_bsp.h"

#include <stdio.h>
#include <string.h>
#include <strings.h>
#include <time.h>

#include "esp_app_desc.h"
#include "esp_chip_info.h"
#include "esp_err.h"
#include "esp_event.h"
#include "esp_heap_caps.h"
#include "esp_http_server.h"
#include "esp_log.h"
#include "esp_netif.h"
#include "esp_system.h"
#include "esp_timer.h"
#include "pet_p4_wifi_bsp.h"
#include "pet_p4_input.h"
#include "pet_p4_renderer.h"
#include "pet_p4_panel_chrome.h"
#include "pet_p4_voice_ptt.h"

static const char *TAG = "pet-p4-http";

#define HTTP_MAX_LINE 4096

static httpd_handle_t s_server;
static pet_p4_http_submit_fn s_submit;
static uint32_t s_post_count;

static esp_err_t json_reply(httpd_req_t *req, const char *status, const char *body) {
  httpd_resp_set_type(req, "application/json");
  if (status) httpd_resp_set_status(req, status);
  return httpd_resp_send(req, body, HTTPD_RESP_USE_STRLEN);
}

static esp_err_t health_get(httpd_req_t *req) {
  return json_reply(req, NULL, "{\"ok\":true,\"service\":\"hachimo-p4\"}");
}

static esp_err_t status_get(httpd_req_t *req) {
  char ip[32] = {0};
  bool connected = pet_p4_wifi_is_connected();
  if (connected) (void) pet_p4_wifi_get_ip(ip, sizeof(ip));

  const esp_app_desc_t *app = esp_app_get_description();
  esp_chip_info_t chip;
  esp_chip_info(&chip);

  char body[1152];
  int key_levels[6] = {0};
  pet_p4_input_gpio_levels(key_levels);
  pet_p4_input_joystick_snapshot_t joy;
  pet_p4_input_get_joystick_snapshot(&joy);
  size_t n = (size_t) snprintf(body, sizeof(body),
    "{\"ok\":true,"
    "\"device\":\"%s\","
    "\"fw\":\"%s\","
    "\"chip\":\"rev%d.%d cores=%d\","
    "\"uptime_s\":%llu,"
    "\"wifi\":{\"connected\":%s,\"ip\":\"%s\"},"
    "\"heap\":{\"internal\":%u,\"psram\":%u},"
    "\"state_posts\":%u,"
    "\"buttons\":{\"k1\":%d,\"k2\":%d,\"k3\":%d,\"k4\":%d,\"joyK\":%d,\"bootB\":%d},"
    "\"joystick\":{\"ready\":%s,\"x\":%d,\"y\":%d,\"cx\":%d,\"cy\":%d,"
    "\"minx\":%d,\"maxx\":%d,\"miny\":%d,\"maxy\":%d},"
    "\"now\":%lld,\"transport\":\"wifi-http-route-b\","
    "\"voiceDraft\":%s}",
    PET_P4_WIFI_HOSTNAME,
    app ? app->version : "unknown",
    chip.revision / 100, chip.revision % 100, chip.cores,
    (unsigned long long) (esp_timer_get_time() / 1000000ULL),
    connected ? "true" : "false", ip,
    (unsigned) heap_caps_get_free_size(MALLOC_CAP_INTERNAL),
    (unsigned) heap_caps_get_free_size(MALLOC_CAP_SPIRAM),
    (unsigned) s_post_count,
    key_levels[0], key_levels[1], key_levels[2],
    key_levels[3], key_levels[4], key_levels[5],
    joy.ready ? "true" : "false",
    joy.current_x, joy.current_y, joy.center_x, joy.center_y,
    joy.minimum_x, joy.maximum_x, joy.minimum_y, joy.maximum_y,
    (long long) time(NULL),
    pet_p4_voice_ptt_draft_active() ? "true" : "false");
  if (n >= sizeof(body)) n = sizeof(body) - 1;
  return json_reply(req, NULL, body);
}

/* GET /shot —— 把屏幕当前画面导出成 24bit BMP，用于远程自查渲染效果。
 * 内容区 480x640 + 侧栏 480x160 拼成 480x800。数据一次性从 PSRAM 发送。 */
#define SHOT_W 480
#define SHOT_H 800
#define SHOT_CONTENT_H 640

static esp_err_t shot_get(httpd_req_t *req) {
  const uint16_t *content = pet_p4_renderer_screen_framebuffer();
  const uint16_t *chrome = pet_p4_panel_chrome_screen_pixels();
  /* 可选 ?y=<起始行>&h=<行数>：分块抓取，避免 1.15MB 大响应卡住 WiFi 传输 */
  int y0 = 0;
  int rows = SHOT_H;
  char query[64];
  if (httpd_req_get_url_query_str(req, query, sizeof(query)) == ESP_OK) {
    char value[16];
    if (httpd_query_key_value(query, "y", value, sizeof(value)) == ESP_OK) {
      int parsed = atoi(value);
      if (parsed >= 0 && parsed < SHOT_H) y0 = parsed;
    }
    if (httpd_query_key_value(query, "h", value, sizeof(value)) == ESP_OK) {
      int parsed = atoi(value);
      if (parsed > 0) rows = parsed;
    }
  }
  if (y0 + rows > SHOT_H) rows = SHOT_H - y0;
  const size_t row_bytes = (size_t) SHOT_W * 3;
  const size_t pixel_bytes = row_bytes * (size_t) rows;
  const size_t header_bytes = 54;
  const size_t total = header_bytes + pixel_bytes;
  uint8_t *buf = (uint8_t *) heap_caps_malloc(total, MALLOC_CAP_SPIRAM | MALLOC_CAP_8BIT);
  if (!buf) return httpd_resp_send_err(req, HTTPD_500_INTERNAL_SERVER_ERROR, "no mem");
  memset(buf, 0, header_bytes);
  buf[0] = 'B'; buf[1] = 'M';
  uint32_t file_size = (uint32_t) total;
  memcpy(buf + 2, &file_size, 4);
  uint32_t offset = (uint32_t) header_bytes;
  memcpy(buf + 10, &offset, 4);
  uint32_t dib = 40;
  memcpy(buf + 14, &dib, 4);
  int32_t w = SHOT_W, h = (int32_t) rows;
  memcpy(buf + 18, &w, 4);
  memcpy(buf + 22, &h, 4);
  uint16_t planes = 1, bpp = 24;
  memcpy(buf + 26, &planes, 2);
  memcpy(buf + 28, &bpp, 2);
  uint32_t payload = (uint32_t) pixel_bytes;
  memcpy(buf + 34, &payload, 4);
  /* BMP 行序自下而上（只输出 [y0, y0+rows) 这一段） */
  for (int y = 0; y < rows; y += 1) {
    const int sy = y0 + rows - 1 - y;
    uint8_t *dst = buf + header_bytes + (size_t) y * row_bytes;
    for (int x = 0; x < SHOT_W; x += 1) {
      uint16_t px = 0;
      if (sy < SHOT_CONTENT_H) {
        if (content) px = content[(size_t) sy * SHOT_W + x];
      } else if (chrome) {
        px = chrome[(size_t) (sy - SHOT_CONTENT_H) * SHOT_W + x];
      }
      const uint8_t r = (uint8_t) (((px >> 11) & 0x1F) * 255 / 31);
      const uint8_t g = (uint8_t) (((px >> 5) & 0x3F) * 255 / 63);
      const uint8_t b = (uint8_t) ((px & 0x1F) * 255 / 31);
      dst[x * 3 + 0] = b;
      dst[x * 3 + 1] = g;
      dst[x * 3 + 2] = r;
    }
  }
  httpd_resp_set_type(req, "image/bmp");
  httpd_resp_set_hdr(req, "Cache-Control", "no-store");
  /* 1.15MB 一次性 send 会卡在 socket 缓冲上，分段（chunked）推进 */
  esp_err_t err = ESP_OK;
  size_t sent_bytes = 0;
  while (sent_bytes < total) {
    size_t chunk = total - sent_bytes;
    if (chunk > 16384) chunk = 16384;
    err = httpd_resp_send_chunk(req, (const char *) buf + sent_bytes, (ssize_t) chunk);
    if (err != ESP_OK) break;
    sent_bytes += chunk;
  }
  if (err == ESP_OK) err = httpd_resp_send_chunk(req, NULL, 0);
  heap_caps_free(buf);
  return err;
}

/* 读取一行 JSON 并交给协议队列。返回 true 表示已成功入队。 */
static bool submit_body(httpd_req_t *req) {
  size_t total = req->content_len;
  if (total == 0 || total >= HTTP_MAX_LINE) {
    ESP_LOGW(TAG, "拒绝请求：body=%u 字节（上限 %d）", (unsigned) total, HTTP_MAX_LINE - 1);
    return false;
  }
  char *buf = (char *) heap_caps_malloc(total + 1, MALLOC_CAP_SPIRAM | MALLOC_CAP_8BIT);
  if (!buf) buf = (char *) malloc(total + 1);
  if (!buf) return false;

  size_t got = 0;
  while (got < total) {
    int r = httpd_req_recv(req, buf + got, (int) (total - got));
    if (r <= 0) { free(buf); return false; }
    got += (size_t) r;
  }
  buf[got] = 0;
  /* 允许末尾换行（JSON Lines 习惯），但内部不能有换行 */
  while (got && (buf[got - 1] == '\n' || buf[got - 1] == '\r')) buf[--got] = 0;
  if (strchr(buf, '\n') || buf[0] != '{') {
    ESP_LOGW(TAG, "拒绝请求：不是单行 JSON object");
    free(buf);
    return false;
  }

  bool ok = s_submit && s_submit(buf);
  free(buf);
  if (ok) s_post_count += 1;
  return ok;
}

static bool http_request_is_non_browser_json(httpd_req_t *req) {
  /* This is a browser-CSRF guard, not authentication of LAN clients. */
  if (httpd_req_get_hdr_value_len(req, "Origin") != 0) return false;
  char type[64];
  if (httpd_req_get_hdr_value_str(req, "Content-Type", type, sizeof(type)) != ESP_OK) return false;
  static const char json_type[] = "application/json";
  size_t n = sizeof(json_type) - 1;
  return strncasecmp(type, json_type, n) == 0 && (type[n] == '\0' || type[n] == ';');
}

static esp_err_t state_post(httpd_req_t *req) {
  if (!http_request_is_non_browser_json(req)) {
    return json_reply(req, "403 Forbidden", "{\"ok\":false,\"error\":\"browser_origin_or_content_type\"}");
  }
  if (!submit_body(req)) {
    return json_reply(req, "400 Bad Request",
                      "{\"ok\":false,\"error\":\"invalid_or_queue_busy\"}");
  }
  return json_reply(req, NULL, "{\"ok\":true,\"queued\":true}");
}

static esp_err_t options_handler(httpd_req_t *req) {
  return json_reply(req, "403 Forbidden", "{\"ok\":false,\"error\":\"browser_origin\"}");
}

esp_err_t pet_p4_http_init(pet_p4_http_submit_fn submit) {
  if (s_server) return ESP_OK;
  s_submit = submit;

  /* 没配 WiFi 时 netif / 事件循环还没建；httpd 依赖 TCP/IP 栈，这里先补齐。
   * 已存在返回 INVALID_STATE，视为成功。 */
  esp_err_t err = esp_netif_init();
  if (err != ESP_OK && err != ESP_ERR_INVALID_STATE) {
    ESP_LOGE(TAG, "netif 初始化失败: %s", esp_err_to_name(err));
    return err;
  }
  err = esp_event_loop_create_default();
  if (err != ESP_OK && err != ESP_ERR_INVALID_STATE) {
    ESP_LOGE(TAG, "事件循环创建失败: %s", esp_err_to_name(err));
    return err;
  }

  httpd_config_t cfg = HTTPD_DEFAULT_CONFIG();
  cfg.server_port = 80;
  cfg.max_uri_handlers = 8;
  cfg.stack_size = 6144;
  cfg.recv_wait_timeout = 2;
  cfg.send_wait_timeout = 10;
  cfg.task_priority = 8;
  cfg.lru_purge_enable = true;   /* RLCD 教训：连接不释放会堵死 server */

  err = httpd_start(&s_server, &cfg);
  if (err != ESP_OK) {
    ESP_LOGE(TAG, "HTTP server 启动失败: %s", esp_err_to_name(err));
    return err;
  }

  static const httpd_uri_t uris[] = {
    {.uri = "/health", .method = HTTP_GET,    .handler = health_get,      .user_ctx = NULL},
    {.uri = "/status", .method = HTTP_GET,    .handler = status_get,      .user_ctx = NULL},
    {.uri = "/shot",   .method = HTTP_GET,    .handler = shot_get,        .user_ctx = NULL},
    {.uri = "/state",  .method = HTTP_POST,   .handler = state_post,      .user_ctx = NULL},
    {.uri = "/line",   .method = HTTP_POST,   .handler = state_post,      .user_ctx = NULL},
    {.uri = "/*",      .method = HTTP_OPTIONS,.handler = options_handler, .user_ctx = NULL},
  };
  for (size_t i = 0; i < sizeof(uris) / sizeof(uris[0]); i++) {
    esp_err_t reg = httpd_register_uri_handler(s_server, &uris[i]);
    if (reg != ESP_OK) {
      ESP_LOGW(TAG, "注册 %s 失败: %s", uris[i].uri, esp_err_to_name(reg));
    }
  }
  ESP_LOGI(TAG, "HTTP 就绪 :80  /health /status /shot /state /line");
  return ESP_OK;
}

bool pet_p4_http_running(void) {
  return s_server != NULL;
}

uint32_t pet_p4_http_post_count(void) {
  return s_post_count;
}
