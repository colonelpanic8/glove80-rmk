import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(scriptDir, "..");
const layoutPath = path.join(repoRoot, "config", "moergo-layout.json");
const outputPath = path.join(repoRoot, "config", "glove80.keymap");

const layout = JSON.parse(fs.readFileSync(layoutPath, "utf8"));
const rowLengths = [10, 12, 12, 12, 18, 16];
const studioUnlockLayer = "Magic";
const studioUnlockPosition = 64;
const reservedStudioLayerCount = 4;

const layerNames = layout.layer_names.map(sanitizeLayerName);
const syntheticLayers = [
  {
    name: "Games_Mac_Hyper",
    ifLayers: ["Games", "Mac_Hyper"],
    bindings: Array.from({ length: 80 }, () => ({ value: "&trans" })),
  },
];
const allLayerNames = [...layerNames, ...syntheticLayers.map(({ name }) => name)];
const layerIdByIndex = new Map(allLayerNames.map((name, index) => [index, `LAYER_${name}`]));

const bindingAliases = new Map([["&reset", "&sys_reset"]]);

function sanitizeLayerName(name) {
  const sanitized = String(name).replace(/[^A-Za-z0-9_]/g, "_");
  return /^[A-Za-z_]/.test(sanitized) ? sanitized : `L_${sanitized}`;
}

function formatParam(param) {
  const value = String(param.value);
  if (param.params?.length) {
    return `${value}(${param.params.map(formatParam).join(", ")})`;
  }
  if (/^\d+$/.test(value) && layerIdByIndex.has(Number(value))) {
    return layerIdByIndex.get(Number(value));
  }
  return value;
}

function formatBinding(binding) {
  const value = bindingAliases.get(binding.value) ?? binding.value;
  if (value === "&magic") {
    return "&magic LAYER_Magic 0";
  }
  if (!binding.params?.length) {
    return value;
  }
  return [value, ...binding.params.map(formatParam)].join(" ");
}

function formatRows(items, indent = "                ") {
  const rows = [];
  let offset = 0;
  for (const length of rowLengths) {
    const row = items.slice(offset, offset + length);
    rows.push(`${indent}${row.join("  ")}`);
    offset += length;
  }
  return rows.join("\n");
}

function layerBlock(name, index, bindings) {
  const generatedBindings = bindings.map((binding, position) =>
    name === studioUnlockLayer && position === studioUnlockPosition
      ? "&studio_unlock"
      : formatBinding(binding),
  );
  return `        layer_${name} {
            display-name = "${name.replaceAll("_", " ")}";
            bindings = <
${formatRows(generatedBindings)}
            >;
        };`;
}

function generatedLayerDefines() {
  return allLayerNames.map((name, index) => `#define LAYER_${name} ${index}`).join("\n");
}

function generatedKeymapLayers() {
  const sourceLayers = layout.layers
    .map((bindings, index) => layerBlock(layerNames[index], index, bindings))
    .join("\n\n");
  const generatedSyntheticLayers = syntheticLayers
    .map((layer, index) => layerBlock(layer.name, layerNames.length + index, layer.bindings))
    .join("\n\n");
  const reservedStudioLayers = Array.from(
    { length: reservedStudioLayerCount },
    (_, index) => `        studio_reserved_${index + 1} {
            status = "reserved";
        };`,
  ).join("\n\n");
  return [sourceLayers, generatedSyntheticLayers, reservedStudioLayers]
    .filter(Boolean)
    .join("\n\n");
}

function generatedConditionalLayers() {
  return syntheticLayers
    .filter(({ ifLayers }) => ifLayers?.length)
    .map(
      ({ name, ifLayers }) => `        ${name.toLowerCase()} {
            if-layers = <${ifLayers.map((layer) => `LAYER_${layer}`).join(" ")}>;
            then-layer = <LAYER_${name}>;
        };`,
    )
    .join("\n\n");
}

const output = `/*
 * Copyright (c) 2020 The ZMK Contributors
 * Copyright (c) 2023 Innaworks Development Limited, trading as MoErgo
 *
 * SPDX-License-Identifier: MIT
 */

/* Generated from config/moergo-layout.json by scripts/generate-keymap.mjs. */

#undef ZMK_BEHAVIORS_KEEP_ALL

#include <behaviors.dtsi>
#include <dt-bindings/zmk/outputs.h>
#include <dt-bindings/zmk/keys.h>
#include <dt-bindings/zmk/bt.h>
#include <dt-bindings/zmk/rgb.h>

${generatedLayerDefines()}

#ifndef LAYER_Lower
#define LAYER_Lower 0
#endif

/ {
    conditional_layers {
        compatible = "zmk,conditional-layers";

${generatedConditionalLayers()}
    };
};

/ {
    macros {
        rgb_ug_status_macro: rgb_ug_status_macro {
            label = "RGB_UG_STATUS";
            compatible = "zmk,behavior-macro";
            #binding-cells = <0>;
            bindings = <&rgb_ug RGB_STATUS>;
        };
    };
};

/ {
#ifdef BT_DISC_CMD
    behaviors {
        bt_0: bt_0 {
            compatible = "zmk,behavior-tap-dance";
            label = "BT_0";
            #binding-cells = <0>;
            tapping-term-ms = <200>;
            bindings = <&bt_select_0>, <&bt BT_DISC 0>;
        };
        bt_1: bt_1 {
            compatible = "zmk,behavior-tap-dance";
            label = "BT_1";
            #binding-cells = <0>;
            tapping-term-ms = <200>;
            bindings = <&bt_select_1>, <&bt BT_DISC 1>;
        };
        bt_2: bt_2 {
            compatible = "zmk,behavior-tap-dance";
            label = "BT_2";
            #binding-cells = <0>;
            tapping-term-ms = <200>;
            bindings = <&bt_select_2>, <&bt BT_DISC 2>;
        };
        bt_3: bt_3 {
            compatible = "zmk,behavior-tap-dance";
            label = "BT_3";
            #binding-cells = <0>;
            tapping-term-ms = <200>;
            bindings = <&bt_select_3>, <&bt BT_DISC 3>;
        };
    };
    macros {
        bt_select_0: bt_select_0 {
            label = "BT_SELECT_0";
            compatible = "zmk,behavior-macro";
            #binding-cells = <0>;
            bindings = <&out OUT_BLE>, <&bt BT_SEL 0>;
        };
        bt_select_1: bt_select_1 {
            label = "BT_SELECT_1";
            compatible = "zmk,behavior-macro";
            #binding-cells = <0>;
            bindings = <&out OUT_BLE>, <&bt BT_SEL 1>;
        };
        bt_select_2: bt_select_2 {
            label = "BT_SELECT_2";
            compatible = "zmk,behavior-macro";
            #binding-cells = <0>;
            bindings = <&out OUT_BLE>, <&bt BT_SEL 2>;
        };
        bt_select_3: bt_select_3 {
            label = "BT_SELECT_3";
            compatible = "zmk,behavior-macro";
            #binding-cells = <0>;
            bindings = <&out OUT_BLE>, <&bt BT_SEL 3>;
        };
    };
#else
    macros {
        bt_0: bt_0 {
            label = "BT_0";
            compatible = "zmk,behavior-macro";
            #binding-cells = <0>;
            bindings = <&out OUT_BLE>, <&bt BT_SEL 0>;
        };
        bt_1: bt_1 {
            label = "BT_1";
            compatible = "zmk,behavior-macro";
            #binding-cells = <0>;
            bindings = <&out OUT_BLE>, <&bt BT_SEL 1>;
        };
        bt_2: bt_2 {
            label = "BT_2";
            compatible = "zmk,behavior-macro";
            #binding-cells = <0>;
            bindings = <&out OUT_BLE>, <&bt BT_SEL 2>;
        };
        bt_3: bt_3 {
            label = "BT_3";
            compatible = "zmk,behavior-macro";
            #binding-cells = <0>;
            bindings = <&out OUT_BLE>, <&bt BT_SEL 3>;
        };
    };
#endif
};

/ {
    behaviors {
        magic: magic {
            compatible = "zmk,behavior-hold-tap";
            label = "MAGIC_HOLD_TAP";
            #binding-cells = <2>;
            flavor = "tap-preferred";
            tapping-term-ms = <200>;
            bindings = <&mo>, <&rgb_ug_status_macro>;
        };
    };
};

/ {
    keymap {
        compatible = "zmk,keymap";

${generatedKeymapLayers()}
    };
};
`;

fs.writeFileSync(outputPath, output);
