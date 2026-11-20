"""Regression coverage for the followed-Agent indicator in the P4 side rail."""

from pathlib import Path
import re
import subprocess


ROOT = Path(__file__).resolve().parents[1]


def static_function(source: str, name: str) -> str:
    match = re.search(rf"^static\s+[^\n]+\b{re.escape(name)}\(", source, re.MULTILINE)
    assert match, f"missing {name}"
    start = match.start()
    brace = source.index("{", match.end())
    depth = 0
    for index in range(brace, len(source)):
        if source[index] == "{":
            depth += 1
        elif source[index] == "}":
            depth -= 1
            if depth == 0:
                return source[start : index + 1]
    raise AssertionError(f"unterminated {name}")


def test_side_rail_names_the_followed_agent_and_uses_the_agent_badge(tmp_path):
    source = (ROOT / "main/pet_p4_panel_chrome.c").read_text()
    followed = static_function(source, "followed_agent_for_badge")
    helper = static_function(source, "agent_display_name")
    accent = static_function(source, "agent_accent")
    badge = static_function(source, "draw_agent_badge")
    redraw = static_function(source, "redraw")
    harness = f'''\
#include <assert.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#define W 160
#define H 480
#define PET_P4_INPUT_ACTION_MAX 64
#define C_BG 1
#define C_TEXT 2
#define C_DIM 3
#define C_ACCENT 4
#define C_OK 5
#define C_LINE 6
#define C_KEY_RED 7
#define C_KEY_YELLOW 8
#define C_KEY_BLUE 9
#define C_KEY_GREEN 10
#define C_KEY_NEUTRAL 11
typedef struct {{ char active_agent[48], followed_agent[48]; }} pet_p4_runtime_state_t;
static const char *drawn[16];
static int drawn_x[16], drawn_y[16], drawn_count;
static int framed_count;
static void fill(int x,int y,int w,int h,unsigned short c){{(void)x;(void)y;(void)w;(void)h;(void)c;}}
static void frame(int x,int y,int w,int h,unsigned short c){{(void)x;(void)y;(void)w;(void)h;(void)c;++framed_count;}}
static void text(const char *s,int x,int y,unsigned short c,int scale){{(void)c;(void)scale;drawn[drawn_count]=s;drawn_x[drawn_count]=x;drawn_y[drawn_count++]=y;}}
static void text_center(const char *s,int x,int y,unsigned short c,int scale){{text(s,x,y,c,scale);}}
static void draw_bars(int x,int y,int bars){{(void)x;(void)y;(void)bars;}}
static void draw_battery(int x,int y,int pct){{(void)x;(void)y;(void)pct;}}
static void draw_key_row(int x,int y,const char *label,const char *name,const char *gpio,unsigned short color,bool pressed){{(void)x;(void)y;(void)label;(void)name;(void)gpio;(void)color;(void)pressed;}}
static bool pet_p4_input_action_for(const char *event,char *action,unsigned long size){{(void)event;(void)action;(void)size;return false;}}
{followed}
{helper}
{accent}
{badge}
{redraw}
static void clear(void){{drawn_count=framed_count=0;}}
static int contains(const char *value){{for(int i=0;i<drawn_count;i++)if(strcmp(drawn[i],value)==0)return i;return -1;}}
int main(void) {{
  pet_p4_runtime_state_t state={{0}};
  strcpy(state.active_agent,"codex");strcpy(state.followed_agent,"claude-code");
  assert(!strcmp(followed_agent_for_badge(&state),"claude-code"));
  state.followed_agent[0]=0;assert(!strcmp(followed_agent_for_badge(&state),"codex"));
  clear();redraw("10:30","11/13","USB","空闲","主页",80,3,1,0,"codex");
  int label=contains("CODEX");assert(label>=0);assert(drawn_y[label]>=440);assert(framed_count>=1);
  clear();redraw("10:30","11/13","USB","空闲","主页",80,3,1,0,"claude-code");assert(contains("CLAUDE")>=0);
  clear();redraw("10:30","11/13","USB","空闲","主页",80,3,1,0,"workbuddy");assert(contains("WORKBUDDY")>=0);
  clear();redraw("10:30","11/13","USB","空闲","主页",80,3,1,0,"future-agent");assert(contains("AGENT")>=0);
  return 0;
}}
'''
    path = tmp_path / "agent_badge.c"
    executable = tmp_path / "agent_badge"
    path.write_text(harness)
    subprocess.run(
        ["cc", "-std=c11", "-Wall", "-Wextra", "-Werror", "-Wno-unused-parameter", str(path), "-o", str(executable)],
        check=True,
    )
    subprocess.run([str(executable)], check=True)
