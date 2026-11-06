/*
 * [Input] 运行时状态 / 宿主墙钟 / 电池电压。
 * [Output] 160x480 横屏状态栏的 RGB565 绘制与 270 度旋转推送。
 * [Pos] 面板 640~799 行的独占区域，不参与内容区每帧旋转。
 * [Sync] 文案字库由 tools/gen_chrome_font.py 生成 pet_p4_chrome_font.h。
 */

#include "pet_p4_panel_chrome.h"

#include <stdio.h>
#include <string.h>
#include <time.h>

#include "esp_heap_caps.h"
#include "esp_log.h"

#include "pet_p4_chrome_font.h"
#include "pet_p4_input.h"
#include "pet_p4_lcd.h"
#include "pet_p4_protocol.h"
#include "pet_p4_wifi_bsp.h"

static const char *TAG = "chrome";

#define W PET_P4_CHROME_W
#define H PET_P4_CHROME_H

/* 配色（RGB565）：深底 + 暖色强调，和内容区的深空调性一致 */
#define C_BG      0x10C4u  /* #10141C 左右 */
#define C_PANEL   0x1906u
#define C_TEXT    0xEF9Eu
#define C_DIM     0x8C92u
#define C_ACCENT  0xEB32u  /* 暖橙 */
#define C_OK      0x3C4Fu
#define C_LINE    0x3186u
/* 按键识别色：方框填充物理键颜色（1红 2黄 3蓝 4绿），K/B 用中性灰 */
#define C_KEY_RED    0xF800u
#define C_KEY_YELLOW 0xFFE0u
#define C_KEY_BLUE   0x021Fu
#define C_KEY_GREEN  0x07E0u
#define C_KEY_NEUTRAL 0x5AEBu
#define C_KEY_WHITE  0xFFFFu

static uint16_t *s_bar;   /* 160x480 横屏画布 */
static uint16_t *s_rot;   /* 480x160 旋转后缓冲 */
static bool s_dirty = true;
static bool s_ready;

/* 上次画上去的值，用来判断要不要重画 */
static struct {
  char clock[8];
  char date[16];
  char net[20];
  char state[20];
  char page[12];
  int batt;
  int bars;
  int host;            /* 主机（PC）在线 0/1 */
  unsigned int keys;   /* 按键实时电平位图：bit i = 第 i 个键按下（低电平有效） */
} s_shown = {.batt = -999, .bars = -1, .keys = 0xFFFFFFFFu};

/* ── 基础绘制 ─────────────────────────────────────────────────────── */

static inline void put(int x, int y, uint16_t c) {
  if (x < 0 || y < 0 || x >= W || y >= H) return;
  s_bar[(size_t) y * W + x] = c;
}

static void fill(int x, int y, int w, int h, uint16_t c) {
  for (int yy = y; yy < y + h; yy += 1)
    for (int xx = x; xx < x + w; xx += 1) put(xx, yy, c);
}

static void frame(int x, int y, int w, int h, uint16_t c) {
  for (int xx = x; xx < x + w; xx += 1) { put(xx, y, c); put(xx, y + h - 1, c); }
  for (int yy = y; yy < y + h; yy += 1) { put(x, yy, c); put(x + w - 1, yy, c); }
}

/* ASCII 8x16，MSB 在左 */
static int draw_ascii(char ch, int x, int y, uint16_t c, int scale) {
  int idx = (unsigned char) ch - CHROME_ASCII_FIRST;
  if (idx < 0 || idx >= CHROME_ASCII_COUNT) return 8 * scale;
  const uint8_t *rows = chrome_ascii_bits[idx];
  for (int ry = 0; ry < 16; ry += 1) {
    uint8_t bits = rows[ry];
    for (int rx = 0; rx < 8; rx += 1) {
      if ((bits >> (7 - rx)) & 1) fill(x + rx * scale, y + ry * scale, scale, scale, c);
    }
  }
  return 8 * scale;
}

/* 中文字形查表：线性扫描（只有几十个字，比二分还快） */
static const uint8_t *cjk_lookup(uint32_t cp) {
  for (int i = 0; i < CHROME_CJK_COUNT; i += 1) {
    if (chrome_cjk_cp[i] == cp) return chrome_cjk_bits[i];
  }
  return NULL;
}

static int draw_cjk(uint32_t cp, int x, int y, uint16_t c) {
  const uint8_t *g = cjk_lookup(cp);
  if (!g) return 16;
  for (int ry = 0; ry < 16; ry += 1) {
    uint16_t bits = (uint16_t) ((g[ry * 2] << 8) | g[ry * 2 + 1]);
    for (int rx = 0; rx < 16; rx += 1) {
      if ((bits >> (15 - rx)) & 1) put(x + rx, y + ry, c);
    }
  }
  return 16;
}

/* 极简 UTF-8 解码（只处理 1~3 字节，够用） */
static uint32_t utf8_next(const char **p) {
  const unsigned char *s = (const unsigned char *) *p;
  if (!s || !*s) return 0;
  if (s[0] < 0x80) { *p += 1; return s[0]; }
  if ((s[0] & 0xE0) == 0xC0 && s[1]) { *p += 2; return ((s[0] & 0x1F) << 6) | (s[1] & 0x3F); }
  if ((s[0] & 0xF0) == 0xE0 && s[1] && s[2]) {
    *p += 3;
    return ((uint32_t) (s[0] & 0x0F) << 12) | ((uint32_t) (s[1] & 0x3F) << 6) | (s[2] & 0x3F);
  }
  *p += 1;
  return '?';
}

static int text_w(const char *s, int scale) {
  int w = 0;
  const char *p = s;
  while (*p) {
    uint32_t cp = utf8_next(&p);
    if (!cp) break;
    w += (cp < 0x80 ? 8 * scale : 16);
    if (cp < 0x80) w += scale;   /* 半角之间的 1px 间距 */
  }
  return w;
}

static void text(const char *s, int x, int y, uint16_t c, int scale) {
  const char *p = s;
  int cx = x;
  while (*p) {
    uint32_t cp = utf8_next(&p);
    if (!cp) break;
    if (cp < 0x80) {
      cx += draw_ascii((char) cp, cx, y, c, scale) + scale;
    } else {
      cx += draw_cjk(cp, cx, y, c);
    }
  }
}

static void text_center(const char *s, int cx, int y, uint16_t c, int scale) {
  text(s, cx - text_w(s, scale) / 2, y, c, scale);
}

/* ── 小组件 ───────────────────────────────────────────────────────── */

static void draw_battery(int x, int y, int pct) {
  frame(x, y, 30, 15, C_TEXT);
  fill(x + 30, y + 5, 3, 6, C_TEXT);
  if (pct >= 0) {
    int inner = (28 * (pct > 100 ? 100 : pct)) / 100;
    if (inner > 0) fill(x + 1, y + 1, inner, 13, pct <= 20 ? C_ACCENT : C_OK);
  }
}

static void draw_bars(int x, int y, int bars) {
  for (int i = 0; i < 4; i += 1) {
    int h = 3 + i * 3;
    fill(x + i * 5, y + 12 - h, 3, h, i < bars ? C_TEXT : C_LINE);
  }
}

/* 按键行：方框直接填充物理键颜色（1红/2黄/3蓝/4绿，K/B 中性灰），
 * 框内是键名、右侧是用途与 GPIO 号。按下时方框反白（白底+键色字），
 * 用途文字提亮——按哪个键看哪个方框立刻反白，就是接线自检。 */
static void draw_key_row(int x, int y, const char *label, const char *name,
                         const char *gpio, uint16_t color, bool pressed) {
  /* 亮色底（黄/绿）用深色字，暗色底（红/蓝/灰）用亮色字 */
  const uint16_t base_fg =
    (((color >> 5) & 0x3Fu) > ((color >> 11) & 0x1Fu)) ? C_BG : C_TEXT;
  if (pressed) {
    fill(x, y, 15, 15, C_KEY_WHITE);
    frame(x, y, 15, 15, color);
    text_center(label, x + 7, y, color, 1);
    text(name, x + 21, y, C_KEY_WHITE, 1);
    text(gpio, x + 92, y, C_TEXT, 1);
    return;
  }
  fill(x, y, 15, 15, color);
  text_center(label, x + 7, y, base_fg, 1);
  text(name, x + 21, y, C_DIM, 1);
  text(gpio, x + 92, y, C_LINE, 1);
}

/* ── 数据换算 ─────────────────────────────────────────────────────── */

static int battery_percent(int mv) {
  if (mv <= 0) return -1;
  static const int v[] = {3300, 3600, 3700, 3800, 3900, 4000, 4100, 4200};
  static const int p[] = {   0,   20,   40,   60,   75,   85,   95,  100};
  if (mv <= v[0]) return 0;
  if (mv >= v[7]) return 100;
  for (int i = 1; i < 8; i += 1) {
    if (mv <= v[i]) {
      int dv = v[i] - v[i - 1], dp = p[i] - p[i - 1];
      return p[i - 1] + (mv - v[i - 1]) * dp / dv;
    }
  }
  return 100;
}

/* epoch ms → 北京时间（UTC+8）时/分、月/日、星期 */
static void split_local_time(unsigned long long epoch_ms, char *clock, size_t ck,
                             char *date, size_t dt) {
  if (epoch_ms == 0) {
    snprintf(clock, ck, "--:--");
    snprintf(date, dt, "--/--");
    return;
  }
  unsigned long long secs = epoch_ms / 1000ULL + 8ULL * 3600ULL;
  unsigned int sod = (unsigned int) (secs % 86400ULL);
  unsigned long long days = secs / 86400ULL;

  int hh = (int) (sod / 3600), mm = (int) ((sod / 60) % 60);
  /* 1970-01-01 是周四；蔡勒式太绕，这里直接算儒略日余数 */
  int weekday = (int) ((days + 4ULL) % 7ULL);   /* 0=周日 */
  static const char *const wd[] = {"日", "一", "二", "三", "四", "五", "六"};

  /* 公历年月日换算（days since 1970-01-01） */
  unsigned long long z = days + 719468ULL;
  unsigned long long era = z / 146097ULL;
  unsigned int doe = (unsigned int) (z - era * 146097ULL);
  unsigned int yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
  unsigned int y = (unsigned int) (yoe + era * 400ULL);
  unsigned int doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
  unsigned int mp = (5 * doy + 2) / 153;
  unsigned int d = doy - (153 * mp + 2) / 5 + 1;
  unsigned int m = mp < 10 ? mp + 3 : mp - 9;
  if (m <= 2) y += 1;

  snprintf(clock, ck, "%02d:%02d", hh, mm);
  snprintf(date, dt, "%02u/%02u 周%s", m, d, wd[weekday]);
}

static const char *state_word(const pet_p4_runtime_state_t *st, bool connected) {
  if (st->conversation_state[0] && strcmp(st->conversation_state, "ended") != 0) return "对话中";
  const char *s = st->current_state;
  if (!s[0]) return connected ? "空闲" : "断开";
  if (!strcmp(s, "running") || !strcmp(s, "thinking")) return "工作中";
  if (!strcmp(s, "done")) return "完成";
  if (!strcmp(s, "error")) return "错误";
  if (!strcmp(s, "idle")) return "空闲";
  if (!strcmp(s, "waiting")) return "等待";
  return s;   /* 未知状态原样显示（走 ASCII 路径） */
}

static const char *page_word(const pet_p4_runtime_state_t *st) {
  if (!strcmp(st->screen_page, "components")) return "组件";
  if (!strcmp(st->screen_page, "app")) return "应用";
  return "主页";
}

/* ── 整幅重绘 ─────────────────────────────────────────────────────── */

static void redraw(const char *clock, const char *date, const char *net,
                   const char *state, const char *page, int batt, int bars,
                   int host_online, unsigned int keys) {
  fill(0, 0, W, H, C_BG);

  text_center(clock, W / 2, 8, C_TEXT, 2);
  text_center(date, W / 2, 44, C_DIM, 1);
  fill(14, 66, W - 28, 1, C_LINE);

  draw_bars(16, 74, bars);
  text(net, 44, 72, C_TEXT, 1);

  /* 主机（PC）连接状态：谁在跟设备说话 */
  text("主机", 14, 96, C_DIM, 1);
  text(host_online ? "连接" : "断开", 14, 114, host_online ? C_OK : C_DIM, 1);

  text("状态", 14, 136, C_DIM, 1);
  text(state, 14, 154, C_ACCENT, 1);

  text("页面", 14, 176, C_DIM, 1);
  text(page, 14, 194, C_TEXT, 1);

  fill(14, 212, W - 28, 1, C_LINE);

  text("电量", 14, 218, C_DIM, 1);
  draw_battery(14, 238, batt);
  if (batt >= 0) {
    char buf[8];
    snprintf(buf, sizeof(buf), "%d%%", batt);
    text(buf, 50, 237, C_TEXT, 1);
  } else {
    text("USB", 50, 237, C_TEXT, 1);   /* 未接电池：当前由 USB 供电 */
  }

  fill(14, 262, W - 28, 1, C_LINE);

  text("按键", 14, 268, C_DIM, 1);
  /* 接线自检视图（物理命名）：方框填键色 —— 1红 2黄 3蓝 4绿，K=摇杆按下
   * （中性灰），B=BOOT（中性灰）。框内键名、右侧用途 + 实际 GPIO 号，
   * 按下时方框反白。 */
  static const char *const labels[] = {"1", "2", "3", "4", "K", "B"};
  static const char *const names[] = {"确认", "组件", "返回", "—", "确认", "—"};
  static const char *const gpios[] = {"29", "30", "25", "24", "28", "35"};
  static const uint16_t colors[] = {
    C_KEY_RED, C_KEY_YELLOW, C_KEY_BLUE, C_KEY_GREEN, C_KEY_NEUTRAL, C_KEY_NEUTRAL
  };
  for (int i = 0; i < 6; i += 1) {
    draw_key_row(14, 288 + i * 22, labels[i], names[i], gpios[i], colors[i],
                 (keys & (1u << i)) != 0);
  }

  fill(0, H - 1, W, 1, C_LINE);
}

/* ── 对外接口 ─────────────────────────────────────────────────────── */

esp_err_t pet_p4_panel_chrome_init(void) {
  if (s_ready) return ESP_OK;
  s_bar = (uint16_t *) heap_caps_calloc((size_t) W * H, sizeof(uint16_t),
                                        MALLOC_CAP_SPIRAM | MALLOC_CAP_8BIT);
  if (!s_bar) s_bar = (uint16_t *) heap_caps_calloc((size_t) W * H, sizeof(uint16_t),
                                                    MALLOC_CAP_8BIT);
  s_rot = (uint16_t *) heap_caps_calloc((size_t) 480 * 160, sizeof(uint16_t),
                                        MALLOC_CAP_SPIRAM | MALLOC_CAP_8BIT);
  if (!s_rot) s_rot = (uint16_t *) heap_caps_calloc((size_t) 480 * 160, sizeof(uint16_t),
                                                    MALLOC_CAP_8BIT);
  if (!s_bar || !s_rot) {
    ESP_LOGE(TAG, "状态栏缓冲分配失败");
    return ESP_ERR_NO_MEM;
  }
  fill(0, 0, W, H, C_BG);
  s_ready = true;
  s_dirty = true;
  ESP_LOGI(TAG, "侧栏缓冲就绪 %dx%d", W, H);
  return ESP_OK;
}

void pet_p4_panel_chrome_tick(const pet_p4_runtime_state_t *state, unsigned long long now_ms) {
  if (!state) return;
  if (!s_ready && pet_p4_panel_chrome_init() != ESP_OK) return;

  char clock[8], date[16];
  /* 时间来源优先级：PC 心跳锚点 → SNTP/系统时钟 → 占位 */
  unsigned long long wall_ms = pet_p4_host_wall_clock_ms(now_ms);
  if (!wall_ms) {
    time_t sys_now = time(NULL);
    if (sys_now > 1700000000) wall_ms = (unsigned long long) sys_now * 1000ULL;
  }
  split_local_time(wall_ms, clock, sizeof(clock), date, sizeof(date));

  const bool connected = state->host_last_seen_ms > 0 &&
                         now_ms >= state->host_last_seen_ms &&
                         now_ms - state->host_last_seen_ms < 5000ULL;
  const bool wifi_up = pet_p4_wifi_is_connected();
  char wifi_label[20];
  pet_p4_wifi_describe(wifi_label, sizeof(wifi_label));
  const char *net = wifi_label[0] ? wifi_label : (connected ? "USB" : "断开");
  const int bars = wifi_up ? 3 : (connected ? 3 : 0);
  const char *word = state_word(state, connected);
  if (!connected) word = wifi_up ? "等待" : "断开";
  const char *page = page_word(state);
  const int batt = battery_percent(pet_p4_battery_millivolts());
  const int host_online = connected ? 1 : 0;

  /* 按键实时电平（自检用）：低位=按下，打包成位图，变化才重绘 */
  int levels[6] = {0};
  pet_p4_input_gpio_levels(levels);
  unsigned int keys = 0;
  for (int i = 0; i < 6; i += 1) {
    if (levels[i] == 0) keys |= 1u << i;
  }

  if (strcmp(s_shown.clock, clock) == 0 && strcmp(s_shown.date, date) == 0 &&
      strcmp(s_shown.net, net) == 0 && strcmp(s_shown.state, word) == 0 &&
      strcmp(s_shown.page, page) == 0 && s_shown.batt == batt && s_shown.bars == bars &&
      s_shown.host == host_online && s_shown.keys == keys) {
    return;
  }

  snprintf(s_shown.clock, sizeof(s_shown.clock), "%s", clock);
  snprintf(s_shown.date, sizeof(s_shown.date), "%s", date);
  snprintf(s_shown.net, sizeof(s_shown.net), "%s", net);
  snprintf(s_shown.state, sizeof(s_shown.state), "%s", word);
  snprintf(s_shown.page, sizeof(s_shown.page), "%s", page);
  s_shown.batt = batt;
  s_shown.bars = bars;
  s_shown.host = host_online;
  s_shown.keys = keys;

  redraw(clock, date, net, word, page, batt, bars, host_online, keys);
  s_dirty = true;
}

void pet_p4_panel_chrome_flush(void) {
  if (!s_ready || !s_dirty) return;

  /* 270 度旋转，与内容区同一套映射：panel_x = 479 - y，panel_y = 640 + x */
  for (int y = 0; y < H; y += 1) {
    const uint16_t *src = s_bar + (size_t) y * W;
    const int px = (H - 1) - y;
    for (int x = 0; x < W; x += 1) {
      s_rot[(size_t) x * 480 + px] = src[x];
    }
  }

  esp_err_t err = pet_p4_lcd_draw_rgb565(0, PET_P4_CHROME_PANEL_Y, 480, W, s_rot);
  if (err != ESP_OK) {
    ESP_LOGW(TAG, "侧栏推送失败: %s", esp_err_to_name(err));
    return;   /* 保持 dirty，下帧重试 */
  }
  s_dirty = false;
}

bool pet_p4_panel_chrome_dirty(void) {
  return s_dirty;
}
