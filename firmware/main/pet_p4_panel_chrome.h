/*
 * [Input] 运行时状态、宿主墙钟、电池电压。
 * [Output] 横屏右侧 160x480 状态栏的绘制与旋转推送。
 * [Pos] 面板上不属于 640x480 内容区的那 160px 专属区域。
 * [Sync] 改这里要同步 PORTING-PLAN-Waveshare-4.3.md §三。
 */

#pragma once

#include <stdbool.h>
#include <stdint.h>

#include "esp_err.h"
#include "pet_p4_protocol.h"

#ifdef __cplusplus
extern "C" {
#endif

/* 横屏状态栏尺寸：宽度 160、高度 480。
 * 旋转 270 度后落在面板第 640~799 行，与内容区（0~639 行）不重叠。 */
#define PET_P4_CHROME_W 160
#define PET_P4_CHROME_H 480
#define PET_P4_CHROME_PANEL_Y 640

/* 分配 PSRAM 缓冲。重复调用安全。 */
esp_err_t pet_p4_panel_chrome_init(void);

/* 按需重绘：数据没变就什么都不做。now_ms 与渲染主循环同一个时基。 */
void pet_p4_panel_chrome_tick(const pet_p4_runtime_state_t *state, unsigned long long now_ms);

/* 有改动才把 160x480 旋转成 480x160 并推一次 (0,640)-(480,800)。 */
void pet_p4_panel_chrome_flush(void);

/* 自上次 flush 起是否有改动（给上层做调度判断）。 */
bool pet_p4_panel_chrome_dirty(void);

#ifdef __cplusplus
}
#endif
