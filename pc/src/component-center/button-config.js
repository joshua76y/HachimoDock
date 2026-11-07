/**
 * [Input] Per-component buttons.json bindings and install-time control-label overrides.
 * [Output] Shared screen/1红·2黄·3蓝·4绿·K·B/joystick option catalog, label
 *          resolution, exact shipped-v7 board-default migration, authoritative
 *          optional global-exit event resolution, and button-config model-v10
 *          signaling. Current installs keep bindings package-owned.
 *          Physical naming (v10): 1=red 2=yellow 3=blue 4=green K=joystick press
 *          B=BOOT; legacy sw1/sw2/sw3/encoder event names stay resolvable so the
 *          shipped component packages keep working.
 * [Pos] component-center contract helper in pc/src/component-center
 * [Sync] If this file changes, update `pc/src/component-center/.folder.md`.
 */

/* 物理键显示名（全项目统一口径）：方框号 + 颜色字 */
export const PHYSICAL_KEY_LABELS = {
  key1: "1红",
  key2: "2黄",
  key3: "3蓝",
  key4: "4绿",
  joyK: "K摇杆",
  boot: "B",
};

export const COMPONENT_CONTROL_OPTIONS = [
  {
    label: "屏幕点击",
    shortLabel: "点击",
    control: "屏幕区域",
    event: "screen.region.tap",
    help: "点负一屏触发这个组件动作。",
  },
  {
    label: "屏幕长按",
    shortLabel: "长按",
    control: "屏幕区域",
    event: "screen.region.long_press",
    help: "长按负一屏触发这个组件动作。",
  },
  ...["1红", "2黄", "3蓝", "4绿"].map((control, index) => {
    const key = index + 1;
    return {
      label: `${control} 短按`,
      shortLabel: control,
      control,
      event: `button.${key}.short_press`,
      help: `按下设备 ${control} 键后触发这个组件动作。`,
    };
  }),
  {
    label: "1红 长按",
    shortLabel: "1红 长按",
    control: "1红",
    event: "button.1.long_press",
    help: "长按设备 1红 进入或退出歌词模式；全局实时对话或退出绑定优先。",
  },
  {
    label: "K摇杆 短按",
    shortLabel: "K",
    control: "K摇杆",
    event: "button.k.short_press",
    help: "短按摇杆按下键 K 后触发这个组件动作。",
  },
  {
    label: "K摇杆 长按",
    shortLabel: "K 长按",
    control: "K摇杆",
    event: "button.k.long_press",
    help: "长按摇杆按下键 K 后触发这个组件动作。",
  },
  {
    label: "B(BOOT) 短按",
    shortLabel: "B",
    control: "B",
    event: "button.b.short_press",
    help: "短按板载 BOOT 键 B 后触发这个组件动作。",
  },
  {
    label: "摇杆向上",
    shortLabel: "向上",
    control: "前方摇杆",
    event: "joystick.up",
    help: "向上推动摇杆后触发这个组件动作。",
  },
  {
    label: "摇杆向下",
    shortLabel: "向下",
    control: "前方摇杆",
    event: "joystick.down",
    help: "向下推动摇杆后触发这个组件动作。",
  },
  {
    label: "摇杆向右",
    shortLabel: "向右",
    control: "前方摇杆",
    event: "knob.rotate_cw",
    help: "向右推动摇杆后触发这个组件动作；旧包中的顺时针事件继续兼容。",
  },
  {
    label: "摇杆向左",
    shortLabel: "向左",
    control: "前方摇杆",
    event: "knob.rotate_ccw",
    help: "向左推动摇杆后触发这个组件动作；旧包中的逆时针事件继续兼容。",
  },
  {
    label: "摇杆左右方向",
    shortLabel: "左右",
    control: "前方摇杆",
    event: "knob.rotate_cw / knob.rotate_ccw",
    help: "向左或向右推动摇杆都触发同一个组件动作；旧旋钮包继续兼容。",
  },
];

export const P4_COMPONENT_BUTTON_EVENTS = [
  "button.1.short_press",
  "button.2.short_press",
  "button.3.short_press",
  "button.4.short_press",
  "button.k.short_press",
  "button.k.long_press",
  "button.b.short_press",
  "knob.rotate_cw",
  "knob.rotate_ccw",
  "joystick.up",
  "joystick.down",
];

const P4_GLOBAL_BUTTON_EVENTS = new Set([
  ...P4_COMPONENT_BUTTON_EVENTS,
  "button.1.long_press",
  "button.1.hold",
  "button.2.long_press",
  "button.2.hold",
  "button.3.long_press",
  "button.3.hold",
  "button.4.long_press",
  "button.4.hold",
  "button.k.hold",
  "button.b.long_press",
  "button.b.hold",
  /* 旧命名（v9 及更早 / 旧组件包）继续认可，保证历史绑定与包绑定不被误判 */
  "button.sw1.short_press",
  "button.sw1.long_press",
  "button.sw1.hold",
  "button.sw2.short_press",
  "button.sw2.long_press",
  "button.sw2.hold",
  "button.sw3.short_press",
  "button.sw3.long_press",
  "button.sw3.hold",
  "button.encoder.short_press",
  "button.encoder.long_press",
  "button.encoder.hold",
]);

const GLOBAL_EXIT_CONTROL_LABELS = {
  "button.1.long_press": "1红 长按",
  "button.1.hold": "1红 按住",
  "button.2.long_press": "2黄 长按",
  "button.2.hold": "2黄 按住",
  "button.3.long_press": "3蓝 长按",
  "button.3.hold": "3蓝 按住",
  "button.4.long_press": "4绿 长按",
  "button.4.hold": "4绿 按住",
  "button.k.hold": "K 按住",
  "button.b.long_press": "B 长按",
  "button.b.hold": "B 按住",
  "button.sw1.long_press": "SW1 长按",
  "button.sw1.hold": "SW1 按住",
  "button.sw2.long_press": "SW2 长按",
  "button.sw2.hold": "SW2 按住",
  "button.sw3.long_press": "SW3 长按",
  "button.sw3.hold": "SW3 按住",
  "button.encoder.hold": "中键按住",
};

/* v10 按键命名统一：旧事件名（sw1~sw3/encoder）→ 新事件名前缀。
 * 旧组件包与历史绑定仍以旧名声明，查找/显示时先归一再匹配。 */
const V9_TO_V10_EVENT_PREFIXES = {
  "button.sw1.": "button.1.",
  "button.sw2.": "button.2.",
  "button.sw3.": "button.3.",
  "button.encoder.": "button.k.",
};

export function canonicalButtonEvent(event) {
  const value = String(event || "").trim();
  for (const [oldPrefix, newPrefix] of Object.entries(V9_TO_V10_EVENT_PREFIXES)) {
    if (value.startsWith(oldPrefix)) return `${newPrefix}${value.slice(oldPrefix.length)}`;
  }
  return value;
}

export const DEVICE_BUTTON_CONFIG_STORAGE_KEY = "pet-manager.board-voice-config";
export const DEVICE_BUTTON_CONFIG_MODEL_VERSION = 11;

export function resolveGlobalExitEvents(response = {}) {
  const boardConfig = response?.config && typeof response.config === "object"
    ? response.config
    : response;
  const bindings = Array.isArray(boardConfig?.bindings) ? boardConfig.bindings : [];
  return [...new Set(bindings
    .filter((binding) => String(binding?.action || "").trim() === "page_back")
    .map((binding) => String(binding?.event || "").trim())
    .filter((event) => P4_GLOBAL_BUTTON_EVENTS.has(event)))];
}

export function globalExitControlLabel(event) {
  const normalized = String(event || "").trim();
  return COMPONENT_CONTROL_OPTIONS.find((option) => option.event === normalized)?.shortLabel
    || GLOBAL_EXIT_CONTROL_LABELS[normalized]
    || normalized;
}

const P4_V7_SHIPPED_DEFAULT_BINDINGS = {
  "button.sw1.short_press": "page_back",
  "button.sw1.long_press": "disabled",
  "button.sw1.hold": "voice_ptt",
  "button.sw2.short_press": "component_center",
  "button.sw2.long_press": "disabled",
  "button.sw2.hold": "disabled",
  "button.sw3.short_press": "page_enter",
  "button.sw3.long_press": "disabled",
  "button.sw3.hold": "disabled",
  "button.encoder.short_press": "page_enter",
  "button.encoder.long_press": "disabled",
  "button.encoder.hold": "disabled",
  "knob.rotate_cw": "session_next",
  "knob.rotate_ccw": "session_previous",
  "joystick.up": "disabled",
  "joystick.down": "disabled",
};

export function migrateP4V7ShippedBoardDefaults(response = {}, runtime = "") {
  if (String(runtime || "").trim().toLowerCase() !== "esp-p4") {
    return { response, migrated: false };
  }
  const wrapped = response?.config && typeof response.config === "object";
  const boardConfig = wrapped ? response.config : response;
  const bindings = Array.isArray(boardConfig?.bindings) ? boardConfig.bindings : [];
  const actionsByEvent = new Map(bindings.map((binding) => [
    String(binding?.event || "").trim(),
    String(binding?.action || "").trim(),
  ]));
  const matchesShippedV7Defaults = Object.entries(P4_V7_SHIPPED_DEFAULT_BINDINGS)
    .every(([event, action]) => actionsByEvent.get(event) === action);
  if (!matchesShippedV7Defaults) return { response, migrated: false };

  const migratedBindings = bindings.map((binding) => {
    if (binding?.event === "button.sw1.short_press") {
      return { ...binding, action: "page_enter", value: "" };
    }
    if (binding?.event === "button.sw3.short_press") {
      return { ...binding, action: "page_back", value: "" };
    }
    return binding;
  });
  const migratedConfig = {
    ...boardConfig,
    version: DEVICE_BUTTON_CONFIG_MODEL_VERSION,
    bindings: migratedBindings,
  };
  return {
    response: wrapped ? { ...response, config: migratedConfig } : migratedConfig,
    migrated: true,
  };
}

export function defaultControlLabelForBinding(binding = {}) {
  const event = canonicalButtonEvent(binding.event);
  const control = String(binding.control || "");
  const exactMatch = COMPONENT_CONTROL_OPTIONS.find(
    (option) => option.control === control && option.event === event,
  );
  if (exactMatch) return exactMatch.label;
  const eventMatch = COMPONENT_CONTROL_OPTIONS.find((option) => option.event === event);
  if (eventMatch) return eventMatch.label;
  const controlMatch = COMPONENT_CONTROL_OPTIONS.find((option) => option.control === control);
  return controlMatch?.label || control || COMPONENT_CONTROL_OPTIONS[0].label;
}

export function optionForControlLabel(label) {
  return COMPONENT_CONTROL_OPTIONS.find((option) => option.label === label) || null;
}

export function componentControlOptionAllowed(option, binding, component) {
  /* 歌词长按仍只对音乐/音频组件开放；新旧事件名都算同一个物理键 */
  if (option.event !== "button.1.long_press" && option.event !== "button.sw1.long_press") {
    return true;
  }
  return binding.action === "media.lyrics"
    && (component?.mediaSource === "audio.player" || component?.id === "music-player");
}

/* v10 按键命名统一：设备端事件已改为 button.1..4/k/b，PC 本地缓存里的
 * 旧事件名（sw1~sw3/encoder）在这里迁移，值原样保留。 */
export function migrateP4V10ButtonNaming(response = {}, runtime = "") {
  if (String(runtime || "").trim().toLowerCase() !== "esp-p4") {
    return { response, migrated: false };
  }
  const wrapped = response?.config && typeof response.config === "object";
  const boardConfig = wrapped ? response.config : response;
  const bindings = Array.isArray(boardConfig?.bindings) ? boardConfig.bindings : [];
  let migrated = false;
  const renamed = bindings.map((binding) => {
    const event = String(binding?.event || "").trim();
    for (const [oldPrefix, newPrefix] of Object.entries(V9_TO_V10_EVENT_PREFIXES)) {
      if (event.startsWith(oldPrefix)) {
        migrated = true;
        return { ...binding, event: `${newPrefix}${event.slice(oldPrefix.length)}` };
      }
    }
    return binding;
  });
  if (!migrated) return { response, migrated: false };
  const migratedConfig = {
    ...boardConfig,
    version: DEVICE_BUTTON_CONFIG_MODEL_VERSION,
    bindings: renamed,
  };
  return {
    response: wrapped ? { ...response, config: migratedConfig } : migratedConfig,
    migrated: true,
  };
}

export function componentInputEventSlots(event) {
  if (event === "knob.rotate_cw / knob.rotate_ccw") {
    return ["knob.rotate_cw", "knob.rotate_ccw"];
  }
  return event ? [event] : [];
}
