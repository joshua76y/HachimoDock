/*
 * ESP32-P4 WiFi 接入层实现 —— 从 RLCD/VibeBuddy 的 wifi_bsp.c 移植。
 *
 * 与 S3 版的差异只有两点：
 *  1) esp_wifi.h 由 esp_wifi_remote 组件提供（API 同名，实际经 SDIO 转发到 C6）；
 *  2) netif / event loop / NVS 在本项目里可能已由 OTA、USB 等模块先初始化过，
 *     所以全部改成幂等；尤其 NVS 满了不能像独立项目那样直接 erase —— 那会
 *     擦掉用户配置和配对信息。
 */
#include "pet_p4_wifi_bsp.h"

#include <stdio.h>
#include <string.h>

#include "esp_check.h"
#include "esp_err.h"
#include "esp_event.h"
#include "esp_log.h"
#include "esp_netif.h"
#include "esp_wifi.h"
#include "esp_hosted.h"
#include "nvs_flash.h"
#include "nvs.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"

static const char *TAG = "pet-p4-wifi";

static bool s_connected;
static char s_ip[32];

#define WIFI_NVS_NAMESPACE "pet_wifi"
#define WIFI_SSID_MAX      33
#define WIFI_PASS_MAX      65

static bool s_started;            /* esp_wifi_init 只能调一次 */
static bool s_has_credentials;    /* NVS 里存过凭据 */

/* 🔴 连续失败熔断：esp_hosted_init / esp_wifi_remote_init 内部是 ESP_ERROR_CHECK，
 * 硬件异常（C6 掉线 / SDIO 接触不良）时会直接 abort() 重启，USB 也活不成。
 * 失败计数持久化到 NVS；连续 3 次启动失败后开机自动连直接跳过（保 USB 可用），
 * 拿到 IP 即清零；PC 重新下发 apply-wifi 也会清零（重新给机会）。 */
#define WIFI_FAIL_LIMIT 3
#define WIFI_FAIL_KEY "failcnt"

static uint8_t wifi_failcnt_load(void) {
  nvs_handle_t h;
  uint8_t v = 0;
  if (nvs_open(WIFI_NVS_NAMESPACE, NVS_READONLY, &h) == ESP_OK) {
    (void) nvs_get_u8(h, WIFI_FAIL_KEY, &v);
    nvs_close(h);
  }
  return v;
}

static void wifi_failcnt_save(uint8_t v) {
  nvs_handle_t h;
  if (nvs_open(WIFI_NVS_NAMESPACE, NVS_READWRITE, &h) != ESP_OK) return;
  if (nvs_set_u8(h, WIFI_FAIL_KEY, v) == ESP_OK) (void) nvs_commit(h);
  nvs_close(h);
}

/* 已存在 != 失败。事件循环/netif 是全局单例，重复创建返回 INVALID_STATE。 */
static esp_err_t ensure_event_loop(void) {
  esp_err_t err = esp_event_loop_create_default();
  return err == ESP_ERR_INVALID_STATE ? ESP_OK : err;
}

static esp_err_t ensure_netif(void) {
  esp_err_t err = esp_netif_init();
  return err == ESP_ERR_INVALID_STATE ? ESP_OK : err;
}

/* 本项目 NVS 里存着按键映射 / 配对信息，绝不能因为「满了」就整片擦除。 */
static esp_err_t ensure_nvs(void) {
  esp_err_t err = nvs_flash_init();
  if (err == ESP_ERR_NVS_NO_FREE_PAGES || err == ESP_ERR_NVS_NEW_VERSION_FOUND) {
    ESP_LOGW(TAG, "NVS 需要整理，但保留已有数据，跳过 erase: %s", esp_err_to_name(err));
    return ESP_OK;
  }
  return err;
}

static void on_wifi_event(void *arg, esp_event_base_t base, int32_t id, void *data) {
  (void) arg;
  if (base == WIFI_EVENT && id == WIFI_EVENT_STA_START) {
    esp_wifi_connect();
  } else if (base == WIFI_EVENT && id == WIFI_EVENT_STA_DISCONNECTED) {
    s_connected = false;
    s_ip[0] = 0;
    ESP_LOGW(TAG, "WiFi 断开，2s 后重连");
    vTaskDelay(pdMS_TO_TICKS(2000));
    esp_wifi_connect();
  } else if (base == IP_EVENT && id == IP_EVENT_STA_GOT_IP) {
    ip_event_got_ip_t *ev = (ip_event_got_ip_t *) data;
    snprintf(s_ip, sizeof(s_ip), IPSTR, IP2STR(&ev->ip_info.ip));
    s_connected = true;
    if (wifi_failcnt_load()) wifi_failcnt_save(0);  /* 成功一次即清零熔断计数 */
    ESP_LOGI(TAG, "已联网 hostname=%s IP=%s", PET_P4_WIFI_HOSTNAME, s_ip);
  }
}

esp_err_t pet_p4_wifi_init(const char *ssid, const char *password) {
  if (!ssid || !ssid[0] || !password) return ESP_ERR_INVALID_ARG;

  ESP_RETURN_ON_ERROR(ensure_nvs(), TAG, "NVS 初始化失败");
  ESP_RETURN_ON_ERROR(ensure_netif(), TAG, "netif 初始化失败");
  ESP_RETURN_ON_ERROR(ensure_event_loop(), TAG, "事件循环创建失败");

  esp_netif_t *sta = esp_netif_create_default_wifi_sta();
  if (sta) esp_netif_set_hostname(sta, PET_P4_WIFI_HOSTNAME);

  /* 🔴 必须显式初始化 ESP-Hosted 的 SDIO 传输层。
   * esp_hosted 1.4.* 里 esp_hosted_host_init.c 用的是
   * __attribute__((constructor)) 自动初始化，但 ESP-IDF 启动流程不跑 ELF
   * constructor（实机日志里完全没有 "ESP-Hosted starting"）。漏掉这一步时，
   * esp_wifi_remote_init() 内部的
   *   ESP_ERROR_CHECK(esp_hosted_reconfigure()) -> "Transport not initialized"
   * 会直接 abort()，板子每 3.26 秒重启一次，整个 USB 协议也用不了。
   * esp_hosted_init() 自身幂等（init_done 标志），重复调用返回 ESP_OK。 */
  esp_err_t hosted_err = esp_hosted_init();
  if (hosted_err != ESP_OK) {
    ESP_LOGE(TAG, "ESP-Hosted 传输层初始化失败，跳过 WiFi（USB 不受影响）: %s",
             esp_err_to_name(hosted_err));
    return hosted_err;
  }

  wifi_init_config_t cfg = WIFI_INIT_CONFIG_DEFAULT();
  /* 打印缓冲配置核对：RLCD 曾踩过「sdkconfig.h 是 10、运行期拿到 0」的坑，
   * 根因是增量编译没更新 sdkconfig.h，改配置后必须 clean build。 */
  ESP_LOGI(TAG, "wifi_init static_rx=%d dynamic_rx=%d tx_type=%d static_tx=%d mgmt_sbuf=%d",
           (int) cfg.static_rx_buf_num, (int) cfg.dynamic_rx_buf_num,
           (int) cfg.tx_buf_type, (int) cfg.static_tx_buf_num,
           (int) cfg.mgmt_sbuf_num);
  ESP_RETURN_ON_ERROR(esp_wifi_init(&cfg), TAG, "wifi 初始化失败");

  ESP_RETURN_ON_ERROR(
    esp_event_handler_instance_register(WIFI_EVENT, ESP_EVENT_ANY_ID,
                                        &on_wifi_event, NULL, NULL),
    TAG, "注册 WiFi 事件失败");
  ESP_RETURN_ON_ERROR(
    esp_event_handler_instance_register(IP_EVENT, IP_EVENT_STA_GOT_IP,
                                        &on_wifi_event, NULL, NULL),
    TAG, "注册 IP 事件失败");

  wifi_config_t wc = {0};
  strncpy((char *) wc.sta.ssid, ssid, sizeof(wc.sta.ssid) - 1);
  strncpy((char *) wc.sta.password, password, sizeof(wc.sta.password) - 1);
  wc.sta.threshold.authmode = WIFI_AUTH_OPEN;
  wc.sta.sae_pwe_h2e = WPA3_SAE_PWE_BOTH;

  ESP_RETURN_ON_ERROR(esp_wifi_set_mode(WIFI_MODE_STA), TAG, "设置 STA 模式失败");
  ESP_RETURN_ON_ERROR(esp_wifi_set_config(WIFI_IF_STA, &wc), TAG, "设置 WiFi 配置失败");
  ESP_RETURN_ON_ERROR(esp_wifi_start(), TAG, "启动 WiFi 失败");

  /* RLCD 血泪教训：set_ps 必须在 esp_wifi_start() 之后，否则返回
   * ESP_ERR_WIFI_NOT_STARTED 会中断整个初始化。省电模式会让大块传输
   * 被 DTIM 拖住（语音 / 截图明显卡顿），这里关掉。 */
  esp_err_t ps_err = esp_wifi_set_ps(WIFI_PS_NONE);
  if (ps_err != ESP_OK) {
    ESP_LOGW(TAG, "关闭 WiFi 省电失败（remote 下可能不支持）: %s", esp_err_to_name(ps_err));
  }

  /* 链路异常时驱动会每 100ms 刷一条 `wifi:m f null`，淹没 UART0 协议通道。 */
  esp_log_level_set("wifi", ESP_LOG_ERROR);

  s_started = true;
  ESP_LOGI(TAG, "正在连接 SSID=\"%s\" ...", ssid);
  return ESP_OK;
}

bool pet_p4_wifi_is_connected(void) {
  return s_connected;
}

esp_err_t pet_p4_wifi_get_ip(char *buf, size_t len) {
  if (!s_connected || !buf || len == 0) return ESP_ERR_INVALID_STATE;
  strncpy(buf, s_ip, len - 1);
  buf[len - 1] = 0;
  return ESP_OK;
}


/* ── 配网：PC 端 usb_apply_wifi(ssid, psk) 已经会发 control/apply-wifi，
 *    这里只负责把凭据落 NVS 并让 WiFi 连上，PC 侧一行都不用改。 ────────── */

static esp_err_t wifi_save_credentials(const char *ssid, const char *password) {
  nvs_handle_t handle;
  esp_err_t err = nvs_open(WIFI_NVS_NAMESPACE, NVS_READWRITE, &handle);
  if (err != ESP_OK) return err;
  err = nvs_set_str(handle, "ssid", ssid);
  if (err == ESP_OK) err = nvs_set_str(handle, "pass", password);
  if (err == ESP_OK) err = nvs_commit(handle);
  nvs_close(handle);
  return err;
}

static esp_err_t wifi_load_credentials(char *ssid, size_t ssid_len,
                                      char *pass, size_t pass_len) {
  nvs_handle_t handle;
  esp_err_t err = nvs_open(WIFI_NVS_NAMESPACE, NVS_READONLY, &handle);
  if (err != ESP_OK) return err;
  size_t a = ssid_len, b = pass_len;
  err = nvs_get_str(handle, "ssid", ssid, &a);
  if (err == ESP_OK) err = nvs_get_str(handle, "pass", pass, &b);
  nvs_close(handle);
  if (err == ESP_OK && (!ssid[0] || !pass[0])) err = ESP_ERR_NOT_FOUND;
  return err;
}

static void wifi_fill_config(wifi_config_t *wc, const char *ssid, const char *password) {
  memset(wc, 0, sizeof(*wc));
  strncpy((char *) wc->sta.ssid, ssid, sizeof(wc->sta.ssid) - 1);
  strncpy((char *) wc->sta.password, password, sizeof(wc->sta.password) - 1);
  wc->sta.threshold.authmode = WIFI_AUTH_OPEN;
  wc->sta.sae_pwe_h2e = WPA3_SAE_PWE_BOTH;
}

bool pet_p4_wifi_has_credentials(void) {
  return s_has_credentials;
}

esp_err_t pet_p4_wifi_apply(const char *ssid, const char *password) {
  if (!ssid || !ssid[0] || strlen(ssid) > 32 || !password || strlen(password) > 64) {
    return ESP_ERR_INVALID_ARG;
  }
  ESP_RETURN_ON_ERROR(wifi_save_credentials(ssid, password), TAG, "保存 WiFi 凭据失败");
  s_has_credentials = true;
  wifi_failcnt_save(0);   /* 用户重新配网 = 重新给机会 */
  if (!s_started) return pet_p4_wifi_init(ssid, password);

  /* 已经 esp_wifi_init 过：只换配置重连，不能重复 init。
   * disconnect 在没连上时返回错误，忽略即可。 */
  wifi_config_t wc;
  wifi_fill_config(&wc, ssid, password);
  (void) esp_wifi_disconnect();
  ESP_RETURN_ON_ERROR(esp_wifi_set_config(WIFI_IF_STA, &wc), TAG, "更新 WiFi 配置失败");
  ESP_LOGI(TAG, "凭据已更新，正在重连 SSID=\"%s\"", ssid);
  return esp_wifi_connect();
}

esp_err_t pet_p4_wifi_start_from_nvs(void) {
  char ssid[WIFI_SSID_MAX] = {0};
  char pass[WIFI_PASS_MAX] = {0};
  esp_err_t err = wifi_load_credentials(ssid, sizeof(ssid), pass, sizeof(pass));
  if (err != ESP_OK) {
    s_has_credentials = false;
    return ESP_ERR_NOT_FOUND;   /* 没配过网，静默跳过，不拖累启动 */
  }
  s_has_credentials = true;
  uint8_t failcnt = wifi_failcnt_load();
  if (failcnt >= WIFI_FAIL_LIMIT) {
    ESP_LOGE(TAG, "WiFi 连续失败 %u 次，本次开机跳过自动连（保 USB）。"
             "重新配网或断电换 USB 环境后可恢复", (unsigned) failcnt);
    return ESP_ERR_INVALID_STATE;
  }
  ESP_LOGI(TAG, "从 NVS 读到 WiFi 凭据，SSID=\"%s\"（历史失败 %u 次）", ssid, (unsigned) failcnt);
  err = pet_p4_wifi_init(ssid, pass);
  if (err != ESP_OK) {
    wifi_failcnt_save(failcnt + 1);   /* 下次开机再放弃（计数持久化跨重启） */
  }
  return err;
}

void pet_p4_wifi_describe(char *buf, size_t len) {
  if (!buf || !len) return;
  buf[0] = 0;
  if (s_connected) {
    const char *last = strrchr(s_ip, '.');
    snprintf(buf, len, "WiFi %s", last ? last + 1 : s_ip);
  } else if (s_has_credentials) {
    snprintf(buf, len, "WiFi ...");
  }
}
