/*
 * 设备侧 PTT 语音闭环 —— 移植 C1 阶段（路线 B）。
 *
 * 上游链路：button.sw1.hold -> voice_ptt 绑定 -> pet_p4_audio_capture_start/stop
 *           -> 16kHz PCM 以 base64 逐帧上行 USB -> PC 端云端 ASR。
 * 本模块把这条链在 WiFi 在线时改成本地闭环：
 *
 *   K1 按住 -> ES7210 20ms 帧仍由 audio_task 读取 -> send_audio_chunk 钩子
 *           -> 本模块 PSRAM 线性缓冲（上限 512KB = 16s，超出丢弃）
 *   K1 松开 -> capture_stop -> send_audio_end 钩子 -> 提交任务
 *           -> 整段打包 WAV -> HTTP POST -> PC 桥接 (tools/voice/bridge.py :8901/voice)
 *           -> 本地 SenseVoice 识别 -> {"ok":true,"text":"..."}
 *           -> 包成 speech/text 协议行 -> 经 net_submit_line 入协议队列 -> 屏上气泡
 *
 * 关键约束：
 * - 绝不碰 conversation（实时对话）路径：ptt_capturing 标志只在
 *   pet_p4_voice_ptt_begin() 与 send_audio_end 之间为真。
 * - WiFi 未连接时 begin() 返回 false，input.c 回退上游 USB 流式路径，
 *   插线行为与原厂完全一致。
 * - 识别端点存 NVS("pet_voice"/"url")，可通过 control/apply-voice-url 下发修改。
 */
#include "pet_p4_voice_ptt.h"

#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "cJSON.h"
#include "driver/i2c_master.h"
#include "esp_heap_caps.h"
#include "esp_http_client.h"
#include "esp_log.h"
#include "esp_random.h"
#include "esp_timer.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "nvs.h"

#include "pet_p4_audio.h"
#include "pet_p4_wifi_bsp.h"
#include "pet_p4_voice_wifi.h"
#include "pet_p4_tcp_bsp.h"

#define TAG "pet-p4-ptt"

/* 16kHz mono 16bit = 32,000 B/s；512KB ≈ 16 秒，超过即截断（与上游 30s 上限同思路） */
#define PTT_BUF_BYTES (512 * 1024)
#define PTT_TASK_STACK (8 * 1024)
/* PC may first start the ASR service (20 s) and then transcribe (45 s). */
#define PTT_POST_TIMEOUT_MS 80000

#define PTT_NVS_NAMESPACE "pet_voice"
#define PTT_NVS_KEY_URL "url"

/* round-5 目标冻结：按下瞬间向 PC 发 POST <base>/voice/begin，
 * PC 侧快照当前跟随的 Agent；录完投递前若目标已切换则丢弃本次文本，
 * 与 USB 流式路径 DeviceVoiceContext 的 target_is_current 语义对齐。
 * 通知是尽力而为：失败/超时不影响录音主链路。 */
#define PTT_BEGIN_TIMEOUT_MS 2000
#define PTT_BEGIN_TASK_STACK (6 * 1024)

static pet_p4_voice_ptt_submit_fn g_submit_line;
TaskHandle_t g_ptt_task_handle;   /* header 里 extern，worker 提交时用于唤醒 */
static char g_url[128];
static char g_capture_url[128];       /* frozen for the owned recording */
static char g_capture_sid[17];
static portMUX_TYPE g_url_lock = portMUX_INITIALIZER_UNLOCKED;
static char *g_pcm_buf;                /* PSRAM，begin 时分配 */
static atomic_size_t g_pcm_len;
static atomic_bool g_pcm_truncated;
static atomic_bool g_capturing;        /* 本地 PTT 会话进行中（音频钩子生效区间） */
static atomic_bool g_submit_pending;   /* 缓冲已收尾，等 worker 发送 */
static atomic_bool g_ptt_owned;        /* 从开始采集直到 worker 完成，独占缓冲和目标 */
static atomic_bool g_task_started;
static atomic_bool g_draft_active;       /* 语音草稿待确认：屏蔽摇杆原有设定 */
static atomic_uint_least64_t g_draft_until_ms; /* 兜底自动超时，防 PC 失联永久屏蔽 */
static char g_draft_text[256];           /* 草稿识别文字（临界区保护） */
static portMUX_TYPE g_draft_lock = portMUX_INITIALIZER_UNLOCKED;
static void ptt_show_text(const char *text, size_t pcm_len);
static void ptt_show_progress(size_t pcm_len);

void pet_p4_voice_ptt_init(pet_p4_voice_ptt_submit_fn submit_line) {
  g_submit_line = submit_line;
  /* 端点读取：NVS 优先，默认兜底；读失败不致命 */
  nvs_handle_t h;
  g_url[0] = '\0';
  if (nvs_open(PTT_NVS_NAMESPACE, NVS_READONLY, &h) == ESP_OK) {
    size_t len = sizeof(g_url);
    if (nvs_get_str(h, PTT_NVS_KEY_URL, g_url, &len) != ESP_OK) g_url[0] = '\0';
    nvs_close(h);
  }
  ESP_LOGI(TAG, "voice endpoint: %s", g_url[0] ? "configured" : "current TCP host");
}

esp_err_t pet_p4_voice_ptt_apply_url(const char *url) {
  if (!url || !url[0] || strlen(url) >= sizeof(g_url)) return ESP_ERR_INVALID_ARG;
  /* 只接受 http(s) URL，避免写进垃圾导致后续 POST 崩 */
  if (strncmp(url, "http://", 7) != 0 && strncmp(url, "https://", 8) != 0) {
    return ESP_ERR_INVALID_ARG;
  }
  nvs_handle_t h;
  esp_err_t err = nvs_open(PTT_NVS_NAMESPACE, NVS_READWRITE, &h);
  if (err != ESP_OK) return err;
  err = nvs_set_str(h, PTT_NVS_KEY_URL, url);
  if (err == ESP_OK) err = nvs_commit(h);
  nvs_close(h);
  if (err == ESP_OK) {
    taskENTER_CRITICAL(&g_url_lock);
    memcpy(g_url, url, strlen(url) + 1);
    taskEXIT_CRITICAL(&g_url_lock);
    ESP_LOGI(TAG, "voice endpoint updated");
  }
  return err;
}

void pet_p4_voice_ptt_get_url(char *buf, size_t len) {
  if (!buf || !len) return;
  taskENTER_CRITICAL(&g_url_lock);
  size_t n = strnlen(g_url, sizeof(g_url));
  if (n >= len) n = len - 1;
  memcpy(buf, g_url, n);
  buf[n] = '\0';
  taskEXIT_CRITICAL(&g_url_lock);
}

bool pet_p4_voice_ptt_capturing(void) {
  return atomic_load_explicit(&g_capturing, memory_order_acquire);
}

/* 后台通知任务：POST <base>/voice/begin，用完自删。arg = heap 内 url 副本 */
static void ptt_begin_post_task(void *arg) {
  char *url = (char *) arg;
  esp_http_client_config_t cfg = {
    .url = url,
    .timeout_ms = PTT_BEGIN_TIMEOUT_MS,
    .method = HTTP_METHOD_POST,
    .buffer_size = 1024,
  };
  esp_http_client_handle_t client = esp_http_client_init(&cfg);
  if (client) {
    esp_err_t err = esp_http_client_open(client, 0);
    if (err == ESP_OK) {
      esp_http_client_fetch_headers(client);
      esp_http_client_read(client, NULL, 0); /* 吃掉响应，允许连接复用语义收尾 */
    }
    ESP_LOGD(TAG, "voice/begin notify: %s", esp_err_to_name(err));
    esp_http_client_close(client);
    esp_http_client_cleanup(client);
  } else {
    ESP_LOGD(TAG, "voice/begin client init failed");
  }
  free(url);
  vTaskDelete(NULL);
}

static bool ptt_resolve_endpoint(const char *configured, const char *peer, char *out, size_t len) {
  if (!configured || !peer || !out || !len) return false;
  int n = configured[0] ? snprintf(out, len, "%s", configured)
    : peer[0] ? snprintf(out, len, "http://%s:8901/voice", peer) : -1;
  if (n < 0 || (size_t) n >= len) { out[0] = '\0'; return false; }
  return true;
}

static bool ptt_make_url(const char *base, const char *sid, bool begin, char *out, size_t len) {
  if (!base || !sid || !out || !len || strlen(sid) != 16 || strchr(base, '#')) return false;
  for (size_t i = 0; i < 16; i++)
    if (!((sid[i] >= '0' && sid[i] <= '9') || (sid[i] >= 'a' && sid[i] <= 'f'))) return false;
  const char *query = strchr(base, '?');
  size_t path_len = query ? (size_t) (query - base) : strlen(base);
  if (!path_len || path_len > 127) return false;
  int n = snprintf(out, len, "%.*s%s%s%ssid=%s", (int) path_len, base,
                   begin ? "/begin" : "", query ? query : "", query ? "&" : "?", sid);
  if (n < 0 || (size_t) n >= len) { out[0] = '\0'; return false; }
  return true;
}

/* 由冻结的录音地址和标识推导 begin 通知端点。 */
static char *ptt_begin_url_dup(const char *base, const char *sid) {
  size_t size = strlen(base) + 40;
  char *url = (char *) malloc(size);
  if (!url) return NULL;
  if (!ptt_make_url(base, sid, true, url, size)) { free(url); return NULL; }
  return url;
}

/* 按住开始：一段录音和识别完成之前禁止重用缓冲。 */
pet_p4_voice_ptt_begin_result_t pet_p4_voice_ptt_begin(bool session_queue_empty) {
  if (atomic_load_explicit(&g_capturing, memory_order_acquire)) return PET_P4_VOICE_PTT_STARTED;
  bool expected = false;
  if (!atomic_compare_exchange_strong_explicit(&g_ptt_owned, &expected, true,
                                               memory_order_acq_rel, memory_order_acquire)) {
    ptt_show_text("（上一段语音正在处理，请稍候）", 0);
    return PET_P4_VOICE_PTT_BUSY;
  }
  /* A connected TCP control session carries the same scoped PCM protocol as
   * USB. Use that path so device confirmation and target ownership stay in
   * one state machine; keep HTTP as the no-control-session fallback. Claim
   * the PTT lease first so a still-pending HTTP upload cannot be bypassed. */
  if (pet_p4_tcp_client_connected()) {
    atomic_store_explicit(&g_ptt_owned, false, memory_order_release);
    return PET_P4_VOICE_PTT_UNAVAILABLE;
  }
  if (!pet_p4_wifi_is_connected() || !g_ptt_task_handle) {
    atomic_store_explicit(&g_ptt_owned, false, memory_order_release);
    return PET_P4_VOICE_PTT_UNAVAILABLE;
  }
  char configured[sizeof(g_url)], peer[32] = {0};
  pet_p4_voice_ptt_get_url(configured, sizeof(configured));
  if (!configured[0] && pet_p4_tcp_client_connected())
    (void) pet_p4_voice_wifi_copy_host(peer, sizeof(peer));
  if (!ptt_resolve_endpoint(configured, peer, g_capture_url, sizeof(g_capture_url))) {
    atomic_store_explicit(&g_ptt_owned, false, memory_order_release);
    return PET_P4_VOICE_PTT_UNAVAILABLE;
  }
  snprintf(g_capture_sid, sizeof(g_capture_sid), "%08x%08x",
           (unsigned) esp_random(), (unsigned) esp_random());
  if (!g_pcm_buf) {
    g_pcm_buf = (char *) heap_caps_malloc(PTT_BUF_BYTES, MALLOC_CAP_SPIRAM | MALLOC_CAP_8BIT);
    if (!g_pcm_buf) {
      ESP_LOGW(TAG, "ptt buffer alloc failed, fall back to usb");
      atomic_store_explicit(&g_ptt_owned, false, memory_order_release);
      return PET_P4_VOICE_PTT_UNAVAILABLE;
    }
  }
  atomic_store_explicit(&g_pcm_len, 0, memory_order_relaxed);
  atomic_store_explicit(&g_pcm_truncated, false, memory_order_relaxed);
  atomic_store_explicit(&g_capturing, true, memory_order_release);
  esp_err_t err = pet_p4_audio_capture_start_owned(session_queue_empty, "ptt-local");
  if (err != ESP_OK) {
    atomic_store_explicit(&g_capturing, false, memory_order_release);
    atomic_store_explicit(&g_ptt_owned, false, memory_order_release);
    ESP_LOGW(TAG, "capture_start_owned failed: %s", esp_err_to_name(err));
    return PET_P4_VOICE_PTT_UNAVAILABLE;
  }
  ESP_LOGI(TAG, "local ptt session start");
  /* 目标冻结通知：capture 成功后再发，失败不影响录音 */
  char *begin_url = ptt_begin_url_dup(g_capture_url, g_capture_sid);
  if (begin_url) {
    if (xTaskCreate(ptt_begin_post_task, "ptt-begin", PTT_BEGIN_TASK_STACK,
                    begin_url, 4, NULL) != pdPASS) {
      free(begin_url);
    }
  }
  return PET_P4_VOICE_PTT_STARTED;
}

/* 音频钩子：仅 capturing 区间被 audio.c 调用；超出容量直接截断 */
void pet_p4_voice_ptt_feed(const uint8_t *pcm, size_t len) {
  if (!g_pcm_buf || !pcm || !len) return;
  size_t old = atomic_load_explicit(&g_pcm_len, memory_order_relaxed);
  size_t room = PTT_BUF_BYTES - old;
  size_t take = len < room ? len : room;
  if (take) memcpy(g_pcm_buf + old, pcm, take);
  if (take < len) atomic_store_explicit(&g_pcm_truncated, true, memory_order_relaxed);
  atomic_store_explicit(&g_pcm_len, old + take, memory_order_release);
}

/* 松开/超时/读错误收尾（audio_task send_audio_end 里调用，缓冲此刻已完整） */
void pet_p4_voice_ptt_notify_end(void) {
  if (!atomic_load_explicit(&g_capturing, memory_order_acquire)) return;
  atomic_store_explicit(&g_capturing, false, memory_order_release);
  size_t len = atomic_load_explicit(&g_pcm_len, memory_order_acquire);
  if (atomic_load_explicit(&g_pcm_truncated, memory_order_acquire)) {
    ptt_show_text("（录音过长，请在16秒内重录）", len);
    atomic_store_explicit(&g_ptt_owned, false, memory_order_release);
    return;
  }
  if (len < 3200) { /* <100ms 的误触不值得发一次请求 */
    ESP_LOGI(TAG, "ptt too short (%u B), dropped", (unsigned) len);
    atomic_store_explicit(&g_ptt_owned, false, memory_order_release);
    return;
  }
  ptt_show_progress(len);
  atomic_store_explicit(&g_submit_pending, true, memory_order_release);
  /* 唤醒 worker */
  if (g_ptt_task_handle) xTaskNotifyGive(g_ptt_task_handle);
}

void pet_p4_voice_ptt_abort(void) {
  if (!atomic_exchange_explicit(&g_capturing, false, memory_order_acq_rel)) return;
  ptt_show_text("（录音失败，请重试）", 0);
  atomic_store_explicit(&g_ptt_owned, false, memory_order_release);
}

/* ── WAV 封装（标准 44 字节头） ── */
static void ptt_wav_header(char *out, uint32_t pcm_bytes) {
  const uint32_t sample_rate = PET_P4_AUDIO_SAMPLE_RATE;
  const uint16_t channels = PET_P4_AUDIO_CHANNELS;
  const uint16_t bits = PET_P4_AUDIO_BITS_PER_SAMPLE;
  const uint32_t byte_rate = sample_rate * channels * bits / 8;
  const uint16_t block_align = channels * bits / 8;
  const uint32_t data_len = pcm_bytes;
  const uint32_t riff_len = 36 + data_len;
  memcpy(out + 0, "RIFF", 4);
  memcpy(out + 4, &riff_len, 4);
  memcpy(out + 8, "WAVE", 4);
  memcpy(out + 12, "fmt ", 4);
  const uint32_t fmt_len = 16;
  memcpy(out + 16, &fmt_len, 4);
  const uint16_t fmt_pcm = 1;
  memcpy(out + 20, &fmt_pcm, 2);
  memcpy(out + 22, &channels, 2);
  memcpy(out + 24, &sample_rate, 4);
  memcpy(out + 28, &byte_rate, 4);
  memcpy(out + 32, &block_align, 2);
  memcpy(out + 34, &bits, 2);
  memcpy(out + 36, "data", 4);
  memcpy(out + 40, &data_len, 4);
}

/* 识别文字 -> speech/text 协议行 -> 协议队列 -> 屏上气泡。
 * 字段名与 protocol.md Speech Payload Shape 一致。 */
static void ptt_show_message(const char *text, size_t pcm_len, const char *status) {
  if (!g_submit_line || !text) return;
  cJSON *payload = cJSON_CreateObject();
  if (!payload) return;
  unsigned seconds = (unsigned) (pcm_len / (PET_P4_AUDIO_SAMPLE_RATE * 2));
  char title[32];
  snprintf(title, sizeof(title), "PTT %us", seconds ? seconds : 1);
  cJSON_AddStringToObject(payload, "displayTitle", title);
  cJSON_AddStringToObject(payload, "displayContent", text);
  cJSON_AddStringToObject(payload, "status", status);
  cJSON_AddStringToObject(payload, "statusText", "local asr");
  cJSON_AddNumberToObject(payload, "tsMs", (double) (esp_timer_get_time() / 1000ULL));
  char *payload_str = cJSON_PrintUnformatted(payload);
  cJSON_Delete(payload);
  if (!payload_str) return;
  char *line = (char *) malloc(strlen(payload_str) + 64);
  if (line) {
    snprintf(line, strlen(payload_str) + 64,
             "{\"topic\":\"speech/text\",\"payload\":%s}", payload_str);
    (void) g_submit_line(line);
    free(line);
  }
  cJSON_free(payload_str);
}

static void ptt_show_text(const char *text, size_t pcm_len) {
  ptt_show_message(text, pcm_len, "done");
}

static void ptt_show_progress(size_t pcm_len) {
  ptt_show_message("正在识别，请稍候", pcm_len, "working");
}

/* 草稿确认模式：不再额外弹气泡覆盖识别文字。屏幕提示改为 renderer 在
 * 识别框上方画的一条极轻的「←✕ / ✓→」标记，见 draw_voice_draft_hint()。 */
bool pet_p4_voice_ptt_draft_active(void) {
  if (!atomic_load_explicit(&g_draft_active, memory_order_acquire)) return false;
  const uint64_t now = (uint64_t) (esp_timer_get_time() / 1000ULL);
  if (now > atomic_load_explicit(&g_draft_until_ms, memory_order_relaxed)) {
    atomic_store_explicit(&g_draft_active, false, memory_order_release);
    ESP_LOGW(TAG, "voice draft mode auto-expired (PC notify lost?)");
    return false;
  }
  return true;
}

void pet_p4_voice_ptt_set_draft(bool active, const char *text, const char *hint) {
  (void) hint;
  atomic_store_explicit(&g_draft_active, active, memory_order_release);
  if (active) {
    /* 与 PC 端 VOICE_DRAFT_TTL_MS(180s) 对齐；PC 失联时自动恢复原按键设定 */
    const uint64_t now = (uint64_t) (esp_timer_get_time() / 1000ULL);
    atomic_store_explicit(&g_draft_until_ms, now + 180000ULL, memory_order_relaxed);
    if (text && text[0]) {
      char staged[256];
      snprintf(staged, sizeof(staged), "%s", text);
      taskENTER_CRITICAL(&g_draft_lock);
      memcpy(g_draft_text, staged, sizeof(g_draft_text));
      taskEXIT_CRITICAL(&g_draft_lock);
    } else {
      taskENTER_CRITICAL(&g_draft_lock);
      g_draft_text[0] = '\0';
      taskEXIT_CRITICAL(&g_draft_lock);
    }
    ESP_LOGI(TAG, "voice draft mode ON (caption pinned)");
  } else {
    taskENTER_CRITICAL(&g_draft_lock);
    g_draft_text[0] = '\0';
    taskEXIT_CRITICAL(&g_draft_lock);
    ESP_LOGI(TAG, "voice draft mode OFF (%s)", text ? text : "");
  }
}

void pet_p4_voice_ptt_copy_draft_text(char *out, size_t size) {
  if (!out || size == 0) return;
  taskENTER_CRITICAL(&g_draft_lock);
  size_t len = strnlen(g_draft_text, size - 1);
  memcpy(out, g_draft_text, len);
  out[len] = '\0';
  taskEXIT_CRITICAL(&g_draft_lock);
}

static bool ptt_http_write_all(esp_http_client_handle_t client, const char *data, size_t len) {
  size_t offset = 0;
  while (offset < len) {
    size_t remaining = len - offset;
    int request = (int) (remaining > 4096 ? 4096 : remaining);
    int written = esp_http_client_write(client, data + offset, request);
    if (written <= 0 || written > request) return false;
    offset += (size_t) written;
  }
  return true;
}

static void ptt_worker_task(void *arg) {
  (void) arg;
  while (true) {
    ulTaskNotifyTake(pdTRUE, portMAX_DELAY);
    if (!atomic_load_explicit(&g_submit_pending, memory_order_acquire)) continue;
    atomic_store_explicit(&g_submit_pending, false, memory_order_release);
    if (!g_pcm_buf) {
      atomic_store_explicit(&g_ptt_owned, false, memory_order_release);
      continue;
    }
    size_t pcm_len = atomic_load_explicit(&g_pcm_len, memory_order_acquire);

    /* WAV 头 + PCM 拼成一个缓冲（PSRAM 再借一块，避免原地挪 512KB） */
    char *wav = (char *) heap_caps_malloc(44 + pcm_len, MALLOC_CAP_SPIRAM | MALLOC_CAP_8BIT);
    if (!wav) {
      ESP_LOGW(TAG, "wav alloc failed");
      atomic_store_explicit(&g_ptt_owned, false, memory_order_release);
      continue;
    }
    ptt_wav_header(wav, (uint32_t) pcm_len);
    memcpy(wav + 44, g_pcm_buf, pcm_len);

    uint64_t t0 = (uint64_t) (esp_timer_get_time() / 1000ULL);
    char request_url[sizeof(g_capture_url) + 40];
    if (!ptt_make_url(g_capture_url, g_capture_sid, false, request_url, sizeof(request_url))) {
      heap_caps_free(wav);
      ptt_show_text("（语音地址无效）", pcm_len);
      atomic_store_explicit(&g_ptt_owned, false, memory_order_release);
      continue;
    }
    esp_http_client_config_t cfg = {
      .url = request_url,
      .timeout_ms = PTT_POST_TIMEOUT_MS,
      .method = HTTP_METHOD_POST,
      .buffer_size = 2048,
    };
    esp_http_client_handle_t client = esp_http_client_init(&cfg);
    if (!client) {
      ESP_LOGW(TAG, "http client init failed");
      heap_caps_free(wav);
      atomic_store_explicit(&g_ptt_owned, false, memory_order_release);
      continue;
    }
    esp_http_client_set_header(client, "Content-Type", "audio/wav");
    char text[256] = {0};
    bool ok = false;
    esp_err_t err = esp_http_client_open(client, (int) (44 + pcm_len));
    if (err == ESP_OK) {
      if (ptt_http_write_all(client, wav, 44 + pcm_len)) {
        int content_len = esp_http_client_fetch_headers(client);
        int status = esp_http_client_get_status_code(client);
        char resp[512] = {0};
        int read_total = 0;
        if (content_len > 0) {
          int r;
          while (read_total < (int) sizeof(resp) - 1
                 && (r = esp_http_client_read(client, resp + read_total,
                     (int) sizeof(resp) - 1 - read_total)) > 0) {
            read_total += r;
          }
        }
        if (status == 200) {
          cJSON *root = cJSON_Parse(resp);
          if (root) {
            cJSON *ok_j = cJSON_GetObjectItem(root, "ok");
            cJSON *text_j = cJSON_GetObjectItem(root, "text");
            if (cJSON_IsTrue(ok_j) && cJSON_IsString(text_j) && text_j->valuestring[0]) {
              snprintf(text, sizeof(text), "%s", text_j->valuestring);
              ok = true;
            }
            cJSON_Delete(root);
          }
        }
      }
    }
    esp_http_client_close(client);
    esp_http_client_cleanup(client);
    uint64_t elapsed = (uint64_t) (esp_timer_get_time() / 1000ULL) - t0;
    heap_caps_free(wav);

    if (ok) {
      ESP_LOGI(TAG, "asr ok: %u bytes (%u ms)", (unsigned) strlen(text), (unsigned) elapsed);
      ptt_show_text(text, pcm_len);
    } else {
      ESP_LOGW(TAG, "asr failed err=%s status=%d in %u ms",
               esp_err_to_name(err), err == ESP_OK ? 0 : -1, (unsigned) elapsed);
      ptt_show_text("（识别失败，检查 PC 桥接是否在线）", pcm_len);
    }
    atomic_store_explicit(&g_ptt_owned, false, memory_order_release);
  }
}

void pet_p4_voice_ptt_start_task(void) {
  if (atomic_exchange(&g_task_started, true)) return;
  if (xTaskCreate(ptt_worker_task, "ptt-worker", PTT_TASK_STACK, NULL,
                  5, &g_ptt_task_handle) != pdPASS) {
    atomic_store_explicit(&g_task_started, false, memory_order_release);
    ESP_LOGW(TAG, "ptt worker task create failed");
  }
}
