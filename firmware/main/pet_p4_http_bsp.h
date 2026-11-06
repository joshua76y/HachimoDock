/*
 * 路线 B 的设备侧 HTTP 端点。
 *
 * 目标：让 PC 在**不插 USB** 的情况下也能推状态、查设备。
 * POST /state 收到的 JSON Lines 直接交给现有 g_protocol_queue，
 * 由主循环按原有协议层消费 —— 不新增协议、不改 PC 端契约。
 */
#ifndef PET_P4_HTTP_BSP_H
#define PET_P4_HTTP_BSP_H

#include <stdbool.h>
#include <stdint.h>
#include "esp_err.h"

#ifdef __cplusplus
extern "C" {
#endif

/* 由 main.c 提供：把一行 JSON 送进协议队列。返回 false = 队列忙/未就绪。 */
typedef bool (*pet_p4_http_submit_fn)(const char *line);

esp_err_t pet_p4_http_init(pet_p4_http_submit_fn submit);
bool pet_p4_http_running(void);
uint32_t pet_p4_http_post_count(void);

#ifdef __cplusplus
}
#endif

#endif /* PET_P4_HTTP_BSP_H */
