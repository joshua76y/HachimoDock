/*
 * [Input] Physical-input runtime lifecycle and desktop input-config commands.
 * [Output] Version-10 P4 input configuration API with 16 persisted bindings
 *          (physical layout naming: 1红/2黄/3蓝/4绿/摇杆K/BOOT B) plus a
 *          coherent bounded-try joystick ADC diagnostic snapshot.
 * [Pos] Public interface for the ESP32-P4 input runtime.
 * [Sync] If this file changes, update `firmware/.folder.md` and `protocol.md`.
 */

#pragma once

#include <stdbool.h>

#include "cJSON.h"
#include "esp_err.h"
#include "pet_p4_protocol.h"

#ifdef __cplusplus
extern "C" {
#endif

#define PET_P4_INPUT_CONFIG_VERSION 10
#define PET_P4_INPUT_MAX_BINDINGS 16
#define PET_P4_INPUT_EVENT_MAX 40
#define PET_P4_INPUT_ACTION_MAX 24
#define PET_P4_INPUT_VALUE_MAX 160

typedef struct {
  bool ready;
  int center_x;
  int center_y;
  int current_x;
  int current_y;
  int minimum_x;
  int maximum_x;
  int minimum_y;
  int maximum_y;
} pet_p4_input_joystick_snapshot_t;

esp_err_t pet_p4_input_init(void);
esp_err_t pet_p4_input_reset_config(void);
void pet_p4_input_process(
  pet_p4_runtime_state_t *state,
  pet_p4_send_line_fn send_line,
  void *ctx
);
bool pet_p4_input_handle_config(
  const cJSON *payload,
  bool legacy_button_config,
  pet_p4_send_line_fn send_line,
  void *ctx
);
bool pet_p4_input_send_config_state(
  const cJSON *request,
  const char *board_device_id,
  pet_p4_send_line_fn send_line,
  void *ctx
);
unsigned int pet_p4_input_dropped_events(void);

/* 电池电压（毫伏，已换算成 VBAT）。-1 = 尚未采样到。 */
int pet_p4_battery_millivolts(void);
/* v10 按键命名统一：把新事件名映射到旧组件包声明的兼容名。
 * button.1→button.sw1、button.2→button.sw2、button.3→button.sw3、
 * button.k→button.encoder；button.4/button.b 与摇杆方向无别名，返回 false。
 * 仅在输入任务上下文调用（无锁、无重入）。 */
bool pet_p4_input_legacy_alias(const char *event_name, char *out, size_t out_size);
/* 接线诊断：读 6 个按键 GPIO 的瞬时电平（1=未按，0=按下，上拉输入）。
 * 顺序：key1(红) key2(黄) key3(蓝) key4(绿) joyK bootB。 */
void pet_p4_input_gpio_levels(int levels[6]);
/* Read-only completed observation; no ADC read or input event. Unavailable
 * publication returns zeroed ready=false, never a mixed partially updated row. */
void pet_p4_input_get_joystick_snapshot(pet_p4_input_joystick_snapshot_t *snapshot);

#ifdef __cplusplus
}
#endif
