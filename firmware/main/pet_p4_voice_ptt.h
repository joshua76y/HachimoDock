/*
 * 设备侧 PTT 语音闭环（路线 B）—— 见 pet_p4_voice_ptt.c 头注释。
 *
 * 使用方式：
 *   main.c 启动时：pet_p4_voice_ptt_init(net_submit_line); pet_p4_voice_ptt_start_task();
 *   input.c voice_ptt 绑定 hold_start：pet_p4_voice_ptt_begin(queue_empty)
 *           返回 true 走本地闭环；返回 false 回退上游 USB 流式。
 *   audio.c：send_audio_chunk 开头若 pet_p4_voice_ptt_capturing() 则
 *           pet_p4_voice_ptt_feed() 后直接返回（不上行 USB）；
 *           send_audio_end 里调 pet_p4_voice_ptt_notify_end()。
 */
#ifndef PET_P4_VOICE_PTT_H
#define PET_P4_VOICE_PTT_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include "esp_err.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"

#ifdef __cplusplus
extern "C" {
#endif

/* 协议行提交函数（main.c 注入 net_submit_line，保证走同一个协议队列） */
typedef bool (*pet_p4_voice_ptt_submit_fn)(const char *line);

extern TaskHandle_t g_ptt_task_handle;

void pet_p4_voice_ptt_init(pet_p4_voice_ptt_submit_fn submit_line);
void pet_p4_voice_ptt_start_task(void);

/* 按住开始：WiFi 在线才接管，返回 false 时调用方回退 USB 流式 */
bool pet_p4_voice_ptt_begin(bool session_queue_empty);
bool pet_p4_voice_ptt_capturing(void);

/* 仅 capturing 区间被 audio.c 调用 */
void pet_p4_voice_ptt_feed(const uint8_t *pcm, size_t len);

/* capture 收尾（send_audio_end 内调用） */
void pet_p4_voice_ptt_notify_end(void);

/* 识别端点：NVS("pet_voice"/"url")，control/apply-voice-url 下发 */
esp_err_t pet_p4_voice_ptt_apply_url(const char *url);
void pet_p4_voice_ptt_get_url(char *buf, size_t len);

#ifdef __cplusplus
}
#endif

#endif /* PET_P4_VOICE_PTT_H */
