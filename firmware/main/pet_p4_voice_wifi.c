/*
 * [Input] 采集侧的 16k/1ch/s16le PCM 帧（与 USB JSONL 的 audio/chunk 同一份数据）。
 * [Output] 到 PC 的 TCP 8898 推流：connect / push / stop 三个原语。
 * [Pos] USB 之外的第二条音频通路；不碰识别逻辑，谁喂进来就推谁。
 * [Sync] 端口与格式必须与 pc/src-tauri/src/voice_wifi.rs 一致。
 */

#include "pet_p4_voice_wifi.h"

#include <string.h>

#include "esp_log.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "lwip/netdb.h"
#include "lwip/sockets.h"

static const char *TAG = "voice-wifi";

static int s_sock = -1;

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
  s_sock = sock;
  ESP_LOGI(TAG, "Wi-Fi 音频已连接 %s:%u（16k/1ch/s16le，裸 PCM）", host, (unsigned) port);
  return ESP_OK;
}

bool pet_p4_voice_wifi_active(void) {
  return s_sock >= 0;
}

esp_err_t pet_p4_voice_wifi_push(const void *pcm, size_t bytes) {
  if (s_sock < 0) return ESP_ERR_INVALID_STATE;
  if (!pcm || bytes == 0) return ESP_OK;
  const uint8_t *cursor = (const uint8_t *) pcm;
  size_t left = bytes;
  while (left > 0) {
    const int sent = send(s_sock, cursor, left, 0);
    if (sent <= 0) {
      ESP_LOGW(TAG, "推流失败，断开 Wi-Fi 音频通道");
      pet_p4_voice_wifi_stop();
      return ESP_FAIL;
    }
    cursor += sent;
    left -= (size_t) sent;
  }
  return ESP_OK;
}

void pet_p4_voice_wifi_stop(void) {
  if (s_sock < 0) return;
  /* 关连接就是给 PC 的"这一段说完了"信号，等价于 USB 的 audio/end。 */
  shutdown(s_sock, SHUT_RDWR);
  close(s_sock);
  s_sock = -1;
  ESP_LOGI(TAG, "Wi-Fi 音频通道已关闭（EOF 通知 PC 结束本段）");
}
