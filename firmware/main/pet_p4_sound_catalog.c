/*
 * [Input] 提示音事件名（与 PC 端 prompt_sounds.rs 的 SOUND_EVENTS 一致）。
 * [Output] 事件 → 内置音预设的映射，以及「自定义音在 SPIFFS 上的路径 + 是否存在」
 *          的解析结果。播放层据此决定走内置 preset 还是播 WAV。
 * [Pos] 提示音的可替换层；调用方只需给事件名，不再自己拼路径。
 * [Sync] 事件名改动要同步 pc/src-tauri/src/prompt_sounds.rs。
 */

#include "pet_p4_sound_catalog.h"

#include <stdio.h>
#include <string.h>

#include "esp_log.h"
#include "esp_spiffs.h"

static const char *TAG = "sound-catalog";

typedef struct {
  const char *event;
  unsigned preset;   /* 内置音编号，见 pet_p4_audio_sfx_trigger */
} sound_event_t;

/* 顺序与 PC 端一致：录音开始/结束、识别成功/失败、发送成功、任务完成/失败、
 * 电量偏低、网络断开。preset 只是「没有自定义音时」的兜底，改编号要同步固件。
 * preset 必须落在 0..PET_SFX_PRESETS-1（tap/beep/chirp/low/water/page），
 * 之前映射到 6/7/8 的三个事件越界必然播放失败，已改回有效编号。 */
static const sound_event_t kEvents[] = {
  {"record_start", 0},   /* tap: 60ms 短叩 */
  {"record_stop", 1},    /* beep: 120ms 干净单音 */
  {"asr_ok", 2},         /* chirp: 180ms 上扬 */
  {"asr_fail", 3},       /* low: 300ms 低鸣 */
  {"send_ok", 2},        /* chirp */
  {"task_done", 2},      /* chirp */
  {"task_fail", 3},      /* low */
  {"low_battery", 3},    /* low */
  {"wifi_lost", 5},      /* page: 80ms 纸噪 */
};

#define SOUND_EVENT_COUNT (sizeof(kEvents) / sizeof(kEvents[0]))

static const sound_event_t *find_event(const char *event) {
  if (!event) return NULL;
  for (size_t i = 0; i < SOUND_EVENT_COUNT; i += 1) {
    if (strcmp(kEvents[i].event, event) == 0) return &kEvents[i];
  }
  return NULL;
}

bool pet_p4_sound_catalog_preset(const char *event, unsigned *preset) {
  const sound_event_t *item = find_event(event);
  if (!item) return false;
  if (preset) *preset = item->preset;
  return true;
}

bool pet_p4_sound_catalog_custom_path(const char *event, char *out, size_t out_size) {
  if (!event || !out || out_size == 0) return false;
  const int written = snprintf(out, out_size, "/spiffs/sounds/%s.wav", event);
  return written > 0 && (size_t) written < out_size;
}

bool pet_p4_sound_catalog_has_custom(const char *event) {
  char path[96];
  if (!pet_p4_sound_catalog_custom_path(event, path, sizeof(path))) return false;
  FILE *file = fopen(path, "rb");
  if (!file) return false;
  fclose(file);
  return true;
}

esp_err_t pet_p4_sound_catalog_ensure_dir(void) {
  /* SPIFFS 是扁平命名空间，"目录"只是文件名前缀；这里只做可写性探测，
   * 避免每次播放都尝试创建。 */
  FILE *probe = fopen("/spiffs/sounds/.keep", "ab");
  if (!probe) {
    ESP_LOGW(TAG, "SPIFFS 不可写，提示音将只用内置音");
    return ESP_FAIL;
  }
  fclose(probe);
  return ESP_OK;
}

size_t pet_p4_sound_catalog_count(void) {
  return SOUND_EVENT_COUNT;
}

const char *pet_p4_sound_catalog_event_at(size_t index) {
  return index < SOUND_EVENT_COUNT ? kEvents[index].event : NULL;
}
