/*
 * ESP32-P4 WiFi 接入层 —— 移植路线 B 的第一阶段（P6a）。
 *
 * 背景：ESP32-P4 没有 WiFi 射频，板载 ESP32-C6-MINI-1 经 SDIO 作为
 * ESP-Hosted slave 提供 WiFi6（微雪出厂已预烧 slave 固件）。
 * esp_wifi_remote 保持标准 esp_wifi API 兼容，把调用转发给 C6，
 * 因此这里的代码与 RLCD（ESP32-S3 原生 WiFi）几乎逐字相同。
 *
 * 本阶段只落地「能连上网络」这一层，不启动 HTTP server、不改主循环、
 * 不影响现有 USB 协议通路。PET_P4_WIFI_AUTO_START 默认 0。
 */
#ifndef PET_P4_WIFI_BSP_H
#define PET_P4_WIFI_BSP_H

#include <stdbool.h>
#include <stddef.h>
#include "esp_err.h"

#ifdef __cplusplus
extern "C" {
#endif

/* 开机时自动用 NVS 里的凭据连 WiFi。没有凭据会直接跳过，不影响 USB 通路。 */
#ifndef PET_P4_WIFI_AUTO_START
#define PET_P4_WIFI_AUTO_START 1
#endif

/* hostname，供 PC 端 mDNS / 设备发现用 */
#define PET_P4_WIFI_HOSTNAME "hachimo-p4"

/* 非阻塞：连接结果由事件回调异步更新 */
esp_err_t pet_p4_wifi_init(const char *ssid, const char *password);
bool pet_p4_wifi_is_connected(void);
esp_err_t pet_p4_wifi_get_ip(char *buf, size_t len);

/* ── 配网（走 PC 端现成的 control/apply-wifi 协议，PC 侧无需改动） ──
 * 凭据存在 NVS 命名空间 "pet_wifi" 的 ssid / pass，重启后自动重连。 */
bool pet_p4_wifi_has_credentials(void);
esp_err_t pet_p4_wifi_apply(const char *ssid, const char *password);
esp_err_t pet_p4_wifi_start_from_nvs(void);

/* 侧栏用：已连返回 "WiFi <IP末段>"，有凭据未连返回 "WiFi ..."，
 * 无凭据返回空串（调用方回退到 USB / 断开）。 */
void pet_p4_wifi_describe(char *buf, size_t len);

#ifdef __cplusplus
}
#endif

#endif /* PET_P4_WIFI_BSP_H */
