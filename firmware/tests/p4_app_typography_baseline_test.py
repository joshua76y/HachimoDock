"""Component atlas glyphs share the native fallback font baseline."""

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


def test_atlas_cell_baseline_matches_the_fallback_font_for_mixed_component_text(tmp_path):
    source = (ROOT / "main/pet_p4_renderer.c").read_text()
    helper = static_function(source, "app_font_cell_origin_y")
    harness = f'''\
#include <assert.h>
#include <math.h>
typedef struct {{ int line_height, base_line; }} font_t;
static const font_t pet_p4_font_cn_16_a4 = {{17, 2}};
{helper}
int main(void) {{
  /* Atlas assets are painted at 7/8 of their cell height; fallback glyphs
   * use the actual LVGL baseline (17 - 2) in their 16px design grid. */
  assert(fabsf(app_font_cell_origin_y(10.0f, 32) - 12.0f) < .001f);
  assert(fabsf(app_font_cell_origin_y(10.0f, 18) - 11.125f) < .001f);
  return 0;
}}
'''
    path = tmp_path / "baseline.c"
    executable = tmp_path / "baseline"
    path.write_text(harness)
    subprocess.run(
        ["cc", "-std=c11", "-Wall", "-Wextra", "-Werror", str(path), "-lm", "-o", str(executable)],
        check=True,
    )
    subprocess.run([str(executable)], check=True)
