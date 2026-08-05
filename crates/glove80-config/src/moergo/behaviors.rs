//! Typed view of the behavior tables in a MoErgo Glove80 Layout Editor export.
//!
//! The editor stores custom ZMK behaviors in four sibling arrays next to
//! `layers`: `holdTaps`, `macros`, `combos` and `inputListeners`. Older exports
//! (and anything authored by hand) instead carry a devicetree blob in
//! `custom_defined_behaviors`, which this module deliberately does not parse —
//! [`BehaviorTables::from_layout`] reports it so the caller can say which keys
//! are affected rather than silently importing a keymap with missing behavior.

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Map, Value};

/// A ZMK binding: a behavior reference plus its positional parameters.
///
/// Parameters nest, because a ZMK binding parameter may itself be a behavior
/// (`&macro_param_1to1` wrapping a `&kp`), so this mirrors the editor's own
/// recursive shape rather than flattening to strings.
#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct Binding {
    #[serde(default)]
    pub value: Value,
    #[serde(default)]
    pub params: Vec<Binding>,
}

impl Binding {
    /// The behavior name (`&kp`, `&HRM_left_pinky_v1B_TKZ`), or `""` when the
    /// binding names a bare parameter rather than a behavior.
    pub fn name(&self) -> &str {
        self.value.as_str().unwrap_or_default()
    }

    /// A parameter rendered the way `keyboard.toml` spells it: editor
    /// parameters are either a ZMK keycode string or a bare layer number.
    pub fn param_text(&self, index: usize) -> Option<String> {
        self.params.get(index).map(|param| match &param.value {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        })
    }

    /// The binding rebuilt as editor JSON, so a nested keycode such as
    /// `LS(END)` can go through the same converter the keymap path uses.
    pub fn to_value(&self) -> Value {
        let mut object = serde_json::Map::new();
        object.insert("value".into(), self.value.clone());
        if !self.params.is_empty() {
            object.insert(
                "params".into(),
                Value::Array(self.params.iter().map(Binding::to_value).collect()),
            );
        }
        Value::Object(object)
    }

    pub fn param_u8(&self, index: usize) -> Option<u8> {
        self.params
            .get(index)
            .and_then(|param| param.value.as_u64())
            .and_then(|value| u8::try_from(value).ok())
    }
}

/// A `zmk,behavior-hold-tap` node.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HoldTap {
    pub name: String,
    #[serde(default)]
    pub bindings: Vec<String>,
    pub tapping_term_ms: Option<u32>,
    pub flavor: Option<String>,
    pub quick_tap_ms: Option<u32>,
    pub require_prior_idle_ms: Option<u32>,
    #[serde(default)]
    pub hold_trigger_on_release: bool,
    #[serde(default)]
    pub hold_trigger_key_positions: Vec<usize>,
}

impl HoldTap {
    /// The behavior invoked when the key resolves as a hold.
    pub fn hold_binding(&self) -> &str {
        self.bindings.first().map_or("", String::as_str)
    }

    /// The behavior invoked when the key resolves as a tap. ZMK writes a
    /// hold-tap's bindings as `<&hold>, <&tap>`.
    pub fn tap_binding(&self) -> &str {
        self.bindings.get(1).map_or("", String::as_str)
    }
}

/// A `zmk,behavior-mod-morph` node: one key whose output depends on which
/// modifiers are held. The first case with no `mods` is the default output.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModMorph {
    pub name: String,
    #[serde(default)]
    pub cases: Vec<MorphCase>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MorphCase {
    pub binding: Binding,
    /// ZMK modifier names (`MOD_LSFT`) that select this case. Empty means the
    /// default.
    #[serde(default)]
    pub mods: Vec<String>,
    /// Modifiers passed through to the output rather than consumed.
    #[serde(default)]
    pub keep_mods: Vec<String>,
}

/// A `zmk,behavior-macro` node. `params` is non-empty for the one- and
/// two-parameter macro variants, whose `MACRO_PLACEHOLDER` bindings are filled
/// in from the invoking key.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Macro {
    pub name: String,
    #[serde(default)]
    pub bindings: Vec<Binding>,
    #[serde(default)]
    pub params: Vec<String>,
}

/// A `zmk,combos` child node.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Combo {
    pub name: String,
    /// Absent in a template that carries combos only as metadata to preserve.
    #[serde(default)]
    pub binding: Binding,
    #[serde(default)]
    pub key_positions: Vec<usize>,
    pub timeout_ms: Option<u32>,
    /// Editor layer indices the combo is active on. Absent means every layer.
    pub layers: Option<Vec<i64>>,
}

/// The behavior tables of one export, plus whatever this module refuses to
/// interpret.
#[derive(Clone, Debug, Default)]
pub(crate) struct BehaviorTables {
    pub hold_taps: Vec<HoldTap>,
    pub macros: Vec<Macro>,
    pub combos: Vec<Combo>,
    pub mod_morphs: Vec<ModMorph>,
    /// `custom_defined_behaviors`, when the export carries raw devicetree.
    pub custom_devicetree: Option<String>,
    /// Names of the layers `inputListeners` rescales pointer movement on.
    ///
    /// Rynk's mouse speed is a global interval rather than a per-layer scaler,
    /// so these are reported rather than converted.
    pub scaled_pointer_layers: Vec<usize>,
}

impl BehaviorTables {
    pub fn from_layout(layout: &Map<String, Value>) -> Result<Self> {
        Ok(Self {
            hold_taps: table(layout, "holdTaps")?,
            macros: table(layout, "macros")?,
            combos: table(layout, "combos")?,
            mod_morphs: table(layout, "modMorphs")?,
            custom_devicetree: layout
                .get("custom_defined_behaviors")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_owned),
            scaled_pointer_layers: scaled_pointer_layers(layout),
        })
    }

    pub fn hold_tap(&self, name: &str) -> Option<&HoldTap> {
        self.hold_taps.iter().find(|entry| entry.name == name)
    }

    pub fn mod_morph_named(&self, name: &str) -> Option<&ModMorph> {
        self.mod_morphs.iter().find(|morph| morph.name == name)
    }

    pub fn macro_named(&self, name: &str) -> Option<&Macro> {
        self.macros.iter().find(|entry| entry.name == name)
    }
}

/// The layers any input listener attaches a processor to.
fn scaled_pointer_layers(layout: &Map<String, Value>) -> Vec<usize> {
    let mut layers: Vec<usize> = layout
        .get("inputListeners")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|listener| listener.get("nodes").and_then(Value::as_array))
        .flatten()
        .filter(|node| {
            node.get("inputProcessors")
                .and_then(Value::as_array)
                .is_some_and(|processors| !processors.is_empty())
        })
        .filter_map(|node| node.get("layers").and_then(Value::as_array))
        .flatten()
        .filter_map(Value::as_u64)
        .map(|layer| layer as usize)
        .collect();
    layers.sort_unstable();
    layers.dedup();
    layers
}

fn table<T: for<'de> Deserialize<'de>>(layout: &Map<String, Value>, key: &str) -> Result<Vec<T>> {
    match layout.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(value) => serde_json::from_value(value.clone())
            .with_context(|| format!("MoErgo JSON has an unreadable '{key}' table")),
    }
}
