/**
 * [Input] Widget buttons.json binding records across screen gestures, P4 switches, and joystick input.
 * [Output] User-facing physical-control labels, concise game-play instructions derived from
 *          the actual package bindings, and a component-action allowlist that hides legacy
 *          package-authored system navigation from previews and installs.
 * [Pos] helper node in pc/src/component-center
 * [Sync] If this file changes, update `pc/src/component-center/.folder.md`.
 */

export function formatBindingControl(binding = {}) {
  const event = String(binding.event || "").trim();
  /* v10 物理命名：1红 / 2黄 / 3蓝 / 4绿 / K（摇杆按下）/ B（BOOT）。
   * 旧名 sw1~sw3/encoder 仍可能出现在已安装组件包里，继续给出同一套显示名。 */
  if (event === "button.1.short_press" || event === "button.sw1.short_press") return "1红 · 短按";
  if (event === "button.1.long_press" || event === "button.sw1.long_press") return "1红 · 长按";
  if (event === "button.2.short_press" || event === "button.sw2.short_press") return "2黄 · 短按";
  if (event === "button.2.long_press" || event === "button.sw2.long_press") return "2黄 · 长按";
  if (event === "button.3.short_press" || event === "button.sw3.short_press") return "3蓝 · 短按";
  if (event === "button.3.long_press" || event === "button.sw3.long_press") return "3蓝 · 长按";
  if (event === "button.4.short_press") return "4绿 · 短按";
  if (event === "button.4.long_press") return "4绿 · 长按";
  if (event === "button.k.short_press" || event === "button.encoder.short_press") return "K（摇杆按下）· 短按";
  if (event === "button.k.long_press" || event === "button.encoder.long_press") return "K（摇杆按下）· 长按";
  if (event === "button.b.short_press") return "B（BOOT）· 短按";
  if (event === "button.b.long_press") return "B（BOOT）· 长按";
  if (event === "joystick.up") return "前方摇杆 · 向上";
  if (event === "joystick.down") return "前方摇杆 · 向下";
  if (event === "knob.rotate_cw / knob.rotate_ccw") return "前方摇杆 · 左右";
  if (event === "knob.rotate_cw") return "前方摇杆 · 向右";
  if (event === "knob.rotate_ccw") return "前方摇杆 · 向左";
  if (event === "screen.region.tap") return "负一屏屏幕 · 点击";
  if (event === "screen.region.long_press") return "负一屏屏幕 · 长按";
  return binding.control || event || "未指定";
}

const JOYSTICK_DIRECTION_EVENTS = new Set([
  "joystick.up",
  "joystick.down",
  "knob.rotate_cw",
  "knob.rotate_ccw",
  "knob.rotate_cw / knob.rotate_ccw",
]);

const COMPACT_CONTROL_LABELS = {
  "button.1.short_press": "1红",
  "button.1.long_press": "1红 长按",
  "button.2.short_press": "2黄",
  "button.3.short_press": "3蓝",
  "button.4.short_press": "4绿",
  "button.k.short_press": "K",
  "button.k.long_press": "K 长按",
  "button.b.short_press": "B",
  "button.sw1.short_press": "1红",
  "button.sw1.long_press": "1红 长按",
  "button.sw2.short_press": "2黄",
  "button.sw3.short_press": "3蓝",
  "button.encoder.short_press": "K",
  "button.encoder.long_press": "K 长按",
  "screen.region.tap": "屏幕点击",
  "screen.region.long_press": "屏幕长按",
};

function bindingActionLabel(binding = {}) {
  return String(binding.label || binding.action || "").trim();
}

/** Build one short, mapping-aware sentence for game cards and their detail view. */
export function buildComponentPlayGuide(bindings = []) {
  const routed = (Array.isArray(bindings) ? bindings : [])
    .filter(isRoutedWidgetBinding)
    .filter((binding) => bindingActionLabel(binding));
  if (routed.length === 0) return "";

  const directionLabels = [...new Set(routed
    .filter((binding) => JOYSTICK_DIRECTION_EVENTS.has(String(binding.event || "").trim()))
    .map(bindingActionLabel))];
  const parts = directionLabels.length > 0
    ? [`摇杆：${directionLabels.join("、")}`]
    : [];
  routed
    .filter((binding) => !JOYSTICK_DIRECTION_EVENTS.has(String(binding.event || "").trim()))
    .forEach((binding) => {
      const event = String(binding.event || "").trim();
      const control = COMPACT_CONTROL_LABELS[event] || formatBindingControl(binding);
      parts.push(`${control}：${bindingActionLabel(binding)}`);
    });
  parts.push("退出跟随设备全局设置");
  return parts.join("；");
}

const ROUTED_WIDGET_EVENTS = new Set([
  "screen.region.tap",
  "screen.region.long_press",
  "button.1.short_press",
  "button.2.short_press",
  "button.3.short_press",
  "button.4.short_press",
  "button.k.short_press",
  "button.k.long_press",
  "button.b.short_press",
  "button.sw1.short_press",
  "button.sw2.short_press",
  "button.sw3.short_press",
  "button.encoder.short_press",
  "button.encoder.long_press",
  "knob.rotate_cw",
  "knob.rotate_ccw",
  "joystick.up",
  "joystick.down",
  "knob.rotate_cw / knob.rotate_ccw",
]);

const COMPONENT_SYSTEM_ACTIONS = new Set([
  "page_toggle",
  "page_enter",
  "page_back",
  "page_main",
  "page_app",
  "component_center",
]);

export function isRoutedWidgetBinding(binding = {}) {
  return (
    (ROUTED_WIDGET_EVENTS.has(String(binding.event || "").trim())
      || ((binding.event === "button.1.long_press" || binding.event === "button.sw1.long_press")
        && binding.action === "media.lyrics"))
    && !COMPONENT_SYSTEM_ACTIONS.has(String(binding.action || "").trim())
  );
}
