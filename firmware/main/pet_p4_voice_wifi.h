/*
 * [Input] PC 的 IP 与端口（默认 8898）。
 * [Output] 到 PC 的 HPA1 分帧 PCM 推流：start / push / stop / active。
 * [Pos] 无 USB 时的音频通路；格式 16k/1ch/s16le，结束时校验长度和内容。
 * [Sync] 与 pc/src-tauri/src/voice_wifi.rs 的 VOICE_WIFI_PORT / RATE 对齐。
 */
#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include "esp_err.h"

/* 与 PC 端一致：16 kHz / 单声道 / 16bit little-endian。 */
#define PET_P4_VOICE_WIFI_PORT 8898
#define PET_P4_VOICE_WIFI_RATE 16000

/* 建立连接；已连接时幂等返回 ESP_OK。失败由上层决定回退 USB。 */
esp_err_t pet_p4_voice_wifi_start(const char *host, uint16_t port);

/* 推一帧 PCM。失败会自动关闭通道并返回 ESP_FAIL。 */
esp_err_t pet_p4_voice_wifi_push(const void *pcm, size_t bytes);

/* 发送完成标记并关闭连接；接收端会验证长度和校验值。 */
void pet_p4_voice_wifi_stop(void);
/* Drop an incomplete capture without sending a completion marker. */
void pet_p4_voice_wifi_abort(void);

/* 读 NVS("petcfg"/"voice_host") 自动连接；未配置返回 ESP_ERR_NOT_FOUND。 */
esp_err_t pet_p4_voice_wifi_start_configured(void);

/* 记录 PC 地址（TCP 3333 的连接方），供无人配置时回退。 */
void pet_p4_voice_wifi_note_host(const char *host);
bool pet_p4_voice_wifi_copy_host(char *host, size_t len);

bool pet_p4_voice_wifi_active(void);
