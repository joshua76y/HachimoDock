/*
 * [Input] 采集侧的 16k/1ch/s16le PCM 帧（与 USB JSONL 的 audio/chunk 同一份数据）。
 * [Output] 到 PC 的 TCP 8898 HPA1 分帧推流：connect / push / stop。
 * [Pos] USB 之外的第二条音频通路；不碰识别逻辑，谁喂进来就推谁。
 * [Sync] 端口与格式必须与 pc/src-tauri/src/voice_wifi.rs 一致。
 */

#include "pet_p4_voice_wifi.h"
#include "pet_p4_tcp_bsp.h"

#include <errno.h>
#include <string.h>

#include "esp_log.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "lwip/netdb.h"
#include "lwip/sockets.h"
#include "nvs.h"

static const char *TAG = "voice-wifi";

static int s_sock = -1;
static uint32_t s_total;
static uint32_t s_checksum;
static char s_peer_host[32];   /* 最近一次连上来的 PC 地址 */
static portMUX_TYPE s_peer_lock = portMUX_INITIALIZER_UNLOCKED;
static bool voice_wifi_send_all(int fd, const void *data, size_t len);

esp_err_t pet_p4_voice_wifi_start(const char *host, uint16_t port) {
  if (!host || !*host) return ESP_ERR_INVALID_ARG;
  if (s_sock >= 0) return ESP_OK;   /* 已经在推，幂等 */

  struct sockaddr_in dest = {0};
  dest.sin_family = AF_INET;
  dest.sin_port = htons(port);
  if (inet_pton(AF_INET, host, &dest.sin_addr) != 1) {
    ESP_LOGW(TAG, "主机地址非法：%s", host);
    return ESP_ERR_INVALID_ARG;
  }

  int sock = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
  if (sock < 0) return ESP_FAIL;
  /* 松手时上层直接 close，这个超时只防"PC 卡住"把音频任务拖死。 */
  struct timeval send_timeout = {.tv_sec = 3, .tv_usec = 0};
  setsockopt(sock, SOL_SOCKET, SO_SNDTIMEO, &send_timeout, sizeof(send_timeout));
  if (connect(sock, (struct sockaddr *) &dest, sizeof(dest)) != 0) {
    ESP_LOGW(TAG, "连接 %s:%u 失败，回退 USB/本地播放", host, (unsigned) port);
    close(sock);
    return ESP_FAIL;
  }
  if (!voice_wifi_send_all(sock, "HPA1", 4)) {
    close(sock);
    return ESP_FAIL;
  }
  s_total = 0;
  s_checksum = 0x811c9dc5u;
  s_sock = sock;
  ESP_LOGI(TAG, "Wi-Fi 音频已连接 %s:%u（16k/1ch/s16le，HPA1）", host, (unsigned) port);
  return ESP_OK;
}

static bool voice_wifi_send_all(int fd, const void *data, size_t len) {
  const uint8_t *bytes = (const uint8_t *) data;
  size_t offset = 0;
  while (offset < len) {
    int sent = send(fd, bytes + offset, len - offset, 0);
    if (sent < 0 && errno == EINTR) continue;
    if (sent <= 0 || (size_t) sent > len - offset) return false;
    offset += (size_t) sent;
  }
  return true;
}

static void voice_wifi_close(void) {
  if (s_sock < 0) return;
  shutdown(s_sock, SHUT_RDWR);
  close(s_sock);
  s_sock = -1;
}

bool pet_p4_voice_wifi_active(void) {
  return s_sock >= 0;
}

esp_err_t pet_p4_voice_wifi_push(const void *pcm, size_t bytes) {
  if (s_sock < 0) return ESP_ERR_INVALID_STATE;
  if (bytes == 0) return ESP_OK;
  if (!pcm || bytes > 4096 || (bytes & 1) || bytes > 16u * 1024u * 1024u - s_total) {
    pet_p4_voice_wifi_abort();
    return ESP_FAIL;
  }
  uint8_t header[2] = {(uint8_t) bytes, (uint8_t) (bytes >> 8)};
  if (!voice_wifi_send_all(s_sock, header, sizeof(header)) || !voice_wifi_send_all(s_sock, pcm, bytes)) {
    ESP_LOGW(TAG, "推流失败，丢弃不完整录音");
    pet_p4_voice_wifi_abort();
    return ESP_FAIL;
  }
  const uint8_t *samples = (const uint8_t *) pcm;
  for (size_t i = 0; i < bytes; i++) s_checksum = (s_checksum ^ samples[i]) * 0x01000193u;
  s_total += (uint32_t) bytes;
  return ESP_OK;
}

void pet_p4_voice_wifi_stop(void) {
  if (s_sock < 0) return;
  uint8_t end[10] = {0, 0};
  for (unsigned i = 0; i < 4; i++) {
    end[2 + i] = (uint8_t) (s_total >> (8 * i));
    end[6 + i] = (uint8_t) (s_checksum >> (8 * i));
  }
  (void) voice_wifi_send_all(s_sock, end, sizeof(end));
  voice_wifi_close();
}

void pet_p4_voice_wifi_abort(void) {
  voice_wifi_close();
}

/* 按 NVS 里的 "voice_host" 自动连一次；未配置则返回 ESP_ERR_NOT_FOUND，
 * 由调用方保持原来的 USB 路径。PC 连上设备后写这个键即可切换。 */
/* 记录 PC 的地址：TCP 3333 的连接方就是主机，拔掉 USB 时音频直接推给它，
 * 不需要用户手填 IP。 */
void pet_p4_voice_wifi_note_host(const char *host) {
  if (!host || !*host) return;
  taskENTER_CRITICAL(&s_peer_lock);
  strncpy(s_peer_host, host, sizeof(s_peer_host) - 1);
  s_peer_host[sizeof(s_peer_host) - 1] = '\0';
  taskEXIT_CRITICAL(&s_peer_lock);
}

bool pet_p4_voice_wifi_copy_host(char *host, size_t len) {
  if (!host || !len) return false;
  taskENTER_CRITICAL(&s_peer_lock);
  size_t n = strnlen(s_peer_host, sizeof(s_peer_host));
  bool ok = n > 0 && n < len;
  if (ok) memcpy(host, s_peer_host, n + 1);
  else host[0] = '\0';
  taskEXIT_CRITICAL(&s_peer_lock);
  return ok;
}

esp_err_t pet_p4_voice_wifi_start_configured(void) {
  nvs_handle_t handle;
  char host[32] = {0};
  size_t len = sizeof(host);
  esp_err_t read = nvs_open("petcfg", NVS_READONLY, &handle);
  if (read == ESP_OK) {
    read = nvs_get_str(handle, "voice_host", host, &len);
    nvs_close(handle);
  }
  if (read != ESP_OK || host[0] == '\0') {
    /* 没写 NVS 就用最近连上来的 PC 地址 */
    if (!pet_p4_tcp_client_connected()) return ESP_ERR_NOT_FOUND;
    if (!pet_p4_voice_wifi_copy_host(host, sizeof(host))) return ESP_ERR_NOT_FOUND;
    return pet_p4_voice_wifi_start(host, PET_P4_VOICE_WIFI_PORT);
  }
  return pet_p4_voice_wifi_start(host, PET_P4_VOICE_WIFI_PORT);
}
