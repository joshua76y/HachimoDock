/*
 * 路线 A：USB JSON Lines 协议套 TCP —— 固件侧传输层（见 pet_p4_tcp_bsp.c 头注释）。
 *
 * main.c 用法：
 *   pet_p4_tcp_set_route((const void *)&g_tcp_route);   // 新增 TCP 路由枚举
 *   pet_p4_tcp_init(enqueue_protocol_line, tcp_hello);  // 注入队列入口 + hello
 *   pet_p4_tcp_start_listener();                        // 队列就绪后启动监听 :3333
 *   transport_send_line_impl 的 BROADCAST 分支里调 pet_p4_tcp_broadcast_line(line)。
 */
#ifndef PET_P4_TCP_BSP_H
#define PET_P4_TCP_BSP_H

#include <stdbool.h>
#include "freertos/FreeRTOS.h"
#include "freertos/semphr.h"
#include "pet_p4_protocol.h"

#ifdef __cplusplus
extern "C" {
#endif

/* 协议行入队函数（签名同 main.c 的 enqueue_protocol_line） */
typedef bool (*pet_p4_tcp_enqueue_fn)(
  const char *line,
  pet_p4_send_line_fn send_line,
  void *reply_ctx,
  void *enqueue_ctx
);

/* hello 提供函数：main.c 里包一层 pet_p4_send_hello_for_board(g_hello_board_id, ...) */
typedef void (*pet_p4_tcp_hello_fn)(pet_p4_send_line_fn send_line, void *ctx);

void pet_p4_tcp_init(pet_p4_tcp_enqueue_fn enqueue, pet_p4_tcp_hello_fn hello);
void pet_p4_tcp_set_route(const void *route_ctx);
void pet_p4_tcp_start_listener(void);
bool pet_p4_tcp_client_connected(void);

/* BROADCAST 行转发（main.c transport_send_line_impl 调用） */
void pet_p4_tcp_broadcast_line(const char *line);

#ifdef __cplusplus
}
#endif

#endif /* PET_P4_TCP_BSP_H */
