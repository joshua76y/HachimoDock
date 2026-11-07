/*
 * [Input] 提示音事件名。
 * [Output] 内置音 preset 查询、自定义 WAV 路径与存在性、目录可写探测。
 * [Pos] 固件侧提示音可替换层的公开接口。
 * [Sync] 事件名必须与 pc/src-tauri/src/prompt_sounds.rs 的 SOUND_EVENTS 一致。
 */
#pragma once

#include <stdbool.h>
#include <stddef.h>

#include "esp_err.h"

/* 事件 → 内置音 preset；事件未知时返回 false。 */
bool pet_p4_sound_catalog_preset(const char *event, unsigned *preset);

/* 事件 → SPIFFS 上的自定义音路径（/spiffs/sounds/<event>.wav）。 */
bool pet_p4_sound_catalog_custom_path(const char *event, char *out, size_t out_size);

/* 该事件是否已有自定义音。 */
bool pet_p4_sound_catalog_has_custom(const char *event);

/* 检查 SPIFFS 是否可写（只在初始化时调用一次）。 */
esp_err_t pet_p4_sound_catalog_ensure_dir(void);

size_t pet_p4_sound_catalog_count(void);
const char *pet_p4_sound_catalog_event_at(size_t index);
