use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub const SCHEMA_VERSION: u32 = 1;
pub const KEY_COUNT: usize = 80;
pub const MAX_ZMK_LAYERS: usize = 32;
pub const MIN_EFFECT_PERIOD_MS: u32 = 200;
pub const MAX_EFFECT_PERIOD_MS: u32 = 10_000;
pub const EFFECT_TIME_QUANTUM_MS: u32 = 50;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeConfiguration {
    pub schema_version: u32,
    #[serde(default)]
    pub required_capabilities: RequiredCapabilities,
    pub layers: Vec<RuntimeLayer>,
    #[serde(default)]
    pub lighting_layers: Vec<LightingLayer>,
    #[serde(default)]
    pub toggles: Vec<ToggleDefinition>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequiredCapabilities {
    /// Firmware-owned system state identifiers referenced by this configuration.
    ///
    /// Layer capacity is intentionally not persisted here: the number of layer
    /// records is itself the required capacity, and a target can be checked with
    /// `config validate --layer-capacity`.
    #[serde(default)]
    pub system_states: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeLayer {
    pub id: String,
    pub name: String,
    /// A populated layer is always a dense array of exactly 80 positions.
    /// Empty capacity is represented by the absence of a RuntimeLayer, never by
    /// a reserved/factory/static/dynamic layer record.
    pub bindings: Vec<Binding>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Binding {
    /// Symbolic ZMK behavior name, including its `&` prefix (for example `&kp`).
    pub behavior: String,
    #[serde(default)]
    pub parameters: Vec<BindingParameter>,
}

/// Binding parameters remain compact for common symbols and integers, while a
/// layer reference is explicit so it can be checked before device compilation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BindingParameter {
    Symbol(String),
    Integer(i32),
    Layer(LayerParameter),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LayerParameter {
    pub layer: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LightingLayer {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    pub activation: ActivationPredicate,
    #[serde(default)]
    pub priority: i16,
    /// Sparse by design. Missing key indices are transparent and reveal the
    /// next composed lighting layer.
    #[serde(default)]
    pub cells: BTreeMap<u8, LightingCell>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LightingCell {
    pub color: Rgb,
    #[serde(default)]
    pub effect: LightingEffect,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub enum LightingEffect {
    Static,
    Blink {
        #[serde(rename = "periodMs")]
        period_ms: u32,
        #[serde(rename = "phaseMs", default)]
        phase_ms: u32,
        #[serde(rename = "dutyPercent", default = "default_duty_percent")]
        duty_percent: u8,
    },
    Breathe {
        #[serde(rename = "periodMs")]
        period_ms: u32,
        #[serde(rename = "phaseMs", default)]
        phase_ms: u32,
    },
}

impl Default for LightingEffect {
    fn default() -> Self {
        Self::Static
    }
}

fn default_duty_percent() -> u8 {
    50
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ActivationPredicate {
    Always,
    KeymapLayerActive {
        #[serde(rename = "layerId")]
        layer_id: String,
    },
    Toggle {
        #[serde(rename = "toggleId")]
        toggle_id: String,
    },
    HostSession,
    SystemState {
        #[serde(rename = "stateId")]
        state_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToggleDefinition {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub persistent: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u32);

impl Serialize for Rgb {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&format!("#{:06x}", self.0))
    }
}

struct RgbVisitor;

impl Visitor<'_> for RgbVisitor {
    type Value = Rgb;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an RGB color in #RRGGBB form")
    }

    fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E>
    where
        E: de::Error,
    {
        let hex = value
            .strip_prefix('#')
            .ok_or_else(|| E::custom("RGB color must begin with #"))?;
        if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(E::custom(
                "RGB color must contain exactly six hexadecimal digits",
            ));
        }
        u32::from_str_radix(hex, 16)
            .map(Rgb)
            .map_err(|_| E::custom("invalid RGB color"))
    }
}

impl<'de> Deserialize<'de> for Rgb {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(RgbVisitor)
    }
}

pub fn read_and_validate(
    path: &Path,
    layer_capacity: Option<usize>,
) -> Result<RuntimeConfiguration> {
    let data = fs::read_to_string(path)
        .with_context(|| format!("could not read configuration {}", path.display()))?;
    let configuration: RuntimeConfiguration = serde_json::from_str(&data)
        .with_context(|| format!("could not parse configuration {}", path.display()))?;
    configuration.validate(layer_capacity)?;
    Ok(configuration)
}

impl RuntimeConfiguration {
    pub fn validate(&self, layer_capacity: Option<usize>) -> Result<()> {
        if self.schema_version != SCHEMA_VERSION {
            bail!(
                "unsupported schemaVersion {}; expected {}",
                self.schema_version,
                SCHEMA_VERSION
            );
        }
        if self.layers.is_empty() {
            bail!("configuration must contain at least one layer");
        }
        if self.layers.len() > MAX_ZMK_LAYERS {
            bail!(
                "configuration contains {} layers; ZMK supports at most {MAX_ZMK_LAYERS}",
                self.layers.len()
            );
        }
        if let Some(capacity) = layer_capacity {
            if !(1..=MAX_ZMK_LAYERS).contains(&capacity) {
                bail!("layer capacity must be between 1 and {MAX_ZMK_LAYERS}");
            }
            if self.layers.len() > capacity {
                bail!(
                    "configuration contains {} layers but target capacity is {capacity}",
                    self.layers.len()
                );
            }
        }

        let layer_ids = unique_ids("layer", self.layers.iter().map(|layer| layer.id.as_str()))?;
        unique_ids(
            "lighting layer",
            self.lighting_layers.iter().map(|layer| layer.id.as_str()),
        )?;
        let toggle_ids = unique_ids(
            "toggle",
            self.toggles.iter().map(|toggle| toggle.id.as_str()),
        )?;
        let system_state_ids = unique_ids(
            "required system state",
            self.required_capabilities
                .system_states
                .iter()
                .map(String::as_str),
        )?;

        for layer in &self.layers {
            if layer.name.trim().is_empty() {
                bail!("layer '{}' must have a non-empty name", layer.id);
            }
            if layer.bindings.len() != KEY_COUNT {
                bail!(
                    "layer '{}' has {} bindings; populated layers require exactly {KEY_COUNT}",
                    layer.id,
                    layer.bindings.len()
                );
            }
            for (position, binding) in layer.bindings.iter().enumerate() {
                validate_binding(binding, &layer_ids).with_context(|| {
                    format!("invalid binding {position} in layer '{}'", layer.id)
                })?;
            }
        }

        for toggle in &self.toggles {
            validate_optional_name("toggle", &toggle.id, toggle.name.as_deref())?;
        }
        for lighting in &self.lighting_layers {
            validate_optional_name("lighting layer", &lighting.id, lighting.name.as_deref())?;
            match &lighting.activation {
                ActivationPredicate::Always | ActivationPredicate::HostSession => {}
                ActivationPredicate::KeymapLayerActive { layer_id } => {
                    if !layer_ids.contains(layer_id.as_str()) {
                        bail!(
                            "lighting layer '{}' activates from unknown keymap layer '{layer_id}'",
                            lighting.id
                        );
                    }
                }
                ActivationPredicate::Toggle { toggle_id } => {
                    if !toggle_ids.contains(toggle_id.as_str()) {
                        bail!(
                            "lighting layer '{}' activates from unknown toggle '{toggle_id}'",
                            lighting.id
                        );
                    }
                }
                ActivationPredicate::SystemState { state_id } => {
                    if !system_state_ids.contains(state_id.as_str()) {
                        bail!(
                            "lighting layer '{}' activates from undeclared required system state '{state_id}'",
                            lighting.id
                        );
                    }
                }
            }
            for (key, cell) in &lighting.cells {
                if *key as usize >= KEY_COUNT {
                    bail!(
                        "lighting layer '{}' contains out-of-range key {key}; expected 0..{}",
                        lighting.id,
                        KEY_COUNT - 1
                    );
                }
                validate_effect(&cell.effect).with_context(|| {
                    format!(
                        "invalid effect for key {key} in lighting layer '{}'",
                        lighting.id
                    )
                })?;
            }
        }
        Ok(())
    }
}

fn unique_ids<'a>(kind: &str, ids: impl Iterator<Item = &'a str>) -> Result<HashSet<&'a str>> {
    let mut result = HashSet::new();
    for id in ids {
        validate_id(kind, id)?;
        if !result.insert(id) {
            bail!("duplicate {kind} id '{id}'");
        }
    }
    Ok(result)
}

fn validate_id(kind: &str, id: &str) -> Result<()> {
    let mut chars = id.chars();
    let Some(first) = chars.next() else {
        bail!("{kind} id must not be empty");
    };
    if !first.is_ascii_alphanumeric()
        || !chars
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        bail!(
            "invalid {kind} id '{id}'; use ASCII letters, digits, '-' or '_' and begin with a letter or digit"
        );
    }
    Ok(())
}

fn validate_optional_name(kind: &str, id: &str, name: Option<&str>) -> Result<()> {
    if name.is_some_and(|value| value.trim().is_empty()) {
        bail!("{kind} '{id}' has an empty name");
    }
    Ok(())
}

fn validate_binding(binding: &Binding, layer_ids: &HashSet<&str>) -> Result<()> {
    let behavior = binding.behavior.strip_prefix('&').unwrap_or_default();
    if behavior.is_empty()
        || !behavior
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        bail!(
            "behavior '{}' is not a symbolic &behavior name",
            binding.behavior
        );
    }
    for parameter in &binding.parameters {
        match parameter {
            BindingParameter::Symbol(value) if value.trim().is_empty() => {
                bail!("symbol parameter must not be empty")
            }
            BindingParameter::Layer(reference) if !layer_ids.contains(reference.layer.as_str()) => {
                bail!("binding references unknown layer '{}'", reference.layer)
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_effect(effect: &LightingEffect) -> Result<()> {
    let (period_ms, phase_ms) = match effect {
        LightingEffect::Static => return Ok(()),
        LightingEffect::Blink {
            period_ms,
            phase_ms,
            duty_percent,
        } => {
            if !(1..=99).contains(duty_percent) {
                bail!("blink dutyPercent must be between 1 and 99");
            }
            (*period_ms, *phase_ms)
        }
        LightingEffect::Breathe {
            period_ms,
            phase_ms,
        } => (*period_ms, *phase_ms),
    };
    if !(MIN_EFFECT_PERIOD_MS..=MAX_EFFECT_PERIOD_MS).contains(&period_ms) {
        bail!("effect periodMs must be between {MIN_EFFECT_PERIOD_MS} and {MAX_EFFECT_PERIOD_MS}");
    }
    if period_ms % EFFECT_TIME_QUANTUM_MS != 0 {
        bail!("effect periodMs must be a multiple of {EFFECT_TIME_QUANTUM_MS}");
    }
    if phase_ms >= period_ms {
        bail!("effect phaseMs must be less than periodMs");
    }
    if phase_ms % EFFECT_TIME_QUANTUM_MS != 0 {
        bail!("effect phaseMs must be a multiple of {EFFECT_TIME_QUANTUM_MS}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding() -> Binding {
        Binding {
            behavior: "&kp".into(),
            parameters: vec![BindingParameter::Symbol("A".into())],
        }
    }

    fn valid_configuration() -> RuntimeConfiguration {
        RuntimeConfiguration {
            schema_version: SCHEMA_VERSION,
            required_capabilities: RequiredCapabilities {
                system_states: vec!["low-battery".into()],
            },
            layers: vec![RuntimeLayer {
                id: "base".into(),
                name: "Base".into(),
                bindings: vec![binding(); KEY_COUNT],
            }],
            lighting_layers: vec![LightingLayer {
                id: "base-lighting".into(),
                name: Some("Base lighting".into()),
                activation: ActivationPredicate::Always,
                priority: 0,
                cells: BTreeMap::from([(
                    0,
                    LightingCell {
                        color: Rgb(0x12ab34),
                        effect: LightingEffect::Breathe {
                            period_ms: 1500,
                            phase_ms: 100,
                        },
                    },
                )]),
            }],
            toggles: Vec::new(),
        }
    }

    #[test]
    fn parses_and_validates_canonical_json() {
        let json = serde_json::to_string(&valid_configuration()).unwrap();
        let parsed: RuntimeConfiguration = serde_json::from_str(&json).unwrap();
        parsed.validate(Some(8)).unwrap();
        assert_eq!(parsed.layers[0].bindings.len(), KEY_COUNT);
        assert_eq!(parsed.lighting_layers[0].cells[&0].color, Rgb(0x12ab34));
    }

    #[test]
    fn all_runtime_layers_are_uniform_dense_records() {
        let mut configuration = valid_configuration();
        configuration.layers[0].bindings.pop();
        let error = configuration.validate(None).unwrap_err().to_string();
        assert!(error.contains("exactly 80"), "{error}");

        let mut configuration = valid_configuration();
        configuration.layers.push(RuntimeLayer {
            id: "lower".into(),
            name: "Lower".into(),
            bindings: vec![binding(); KEY_COUNT],
        });
        let error = configuration.validate(Some(1)).unwrap_err().to_string();
        assert!(error.contains("target capacity is 1"), "{error}");
    }

    #[test]
    fn rejects_unknown_schema_versions() {
        let mut configuration = valid_configuration();
        configuration.schema_version = SCHEMA_VERSION + 1;
        let error = configuration.validate(None).unwrap_err().to_string();
        assert!(error.contains("unsupported schemaVersion 2"), "{error}");
    }

    #[test]
    fn rejects_duplicate_ids_and_broken_references() {
        let mut configuration = valid_configuration();
        configuration.layers.push(configuration.layers[0].clone());
        let error = configuration.validate(None).unwrap_err().to_string();
        assert!(error.contains("duplicate layer id"), "{error}");

        let mut configuration = valid_configuration();
        configuration.layers[0].bindings[10].parameters =
            vec![BindingParameter::Layer(LayerParameter {
                layer: "missing".into(),
            })];
        let error = format!("{:#}", configuration.validate(None).unwrap_err());
        assert!(error.contains("unknown layer 'missing'"), "{error}");
    }

    #[test]
    fn validates_every_activation_target() {
        let mut configuration = valid_configuration();
        configuration.lighting_layers[0].activation = ActivationPredicate::Toggle {
            toggle_id: "missing".into(),
        };
        let error = configuration.validate(None).unwrap_err().to_string();
        assert!(error.contains("unknown toggle 'missing'"), "{error}");

        configuration.toggles.push(ToggleDefinition {
            id: "gaming".into(),
            name: None,
            persistent: false,
        });
        configuration.lighting_layers[0].activation = ActivationPredicate::Toggle {
            toggle_id: "gaming".into(),
        };
        configuration.validate(None).unwrap();

        configuration.lighting_layers[0].activation = ActivationPredicate::KeymapLayerActive {
            layer_id: "missing".into(),
        };
        let error = configuration.validate(None).unwrap_err().to_string();
        assert!(error.contains("unknown keymap layer 'missing'"), "{error}");

        configuration.lighting_layers[0].activation = ActivationPredicate::SystemState {
            state_id: "not-required".into(),
        };
        let error = configuration.validate(None).unwrap_err().to_string();
        assert!(
            error.contains("undeclared required system state"),
            "{error}"
        );
    }

    #[test]
    fn validates_rgb_and_effect_timing() {
        let malformed = r##"{"schemaVersion":1,"layers":[],"lightingLayers":[{"id":"x","activation":{"type":"always"},"cells":{"0":{"color":"ff0000"}}}]}"##;
        let error = serde_json::from_str::<RuntimeConfiguration>(malformed)
            .unwrap_err()
            .to_string();
        assert!(error.contains("begin with #"), "{error}");

        let mut configuration = valid_configuration();
        configuration.lighting_layers[0]
            .cells
            .get_mut(&0)
            .unwrap()
            .effect = LightingEffect::Blink {
            period_ms: 525,
            phase_ms: 0,
            duty_percent: 50,
        };
        let error = format!("{:#}", configuration.validate(None).unwrap_err());
        assert!(error.contains("multiple of 50"), "{error}");
    }

    #[test]
    fn sparse_cells_still_enforce_physical_key_range() {
        let mut configuration = valid_configuration();
        configuration.lighting_layers[0].cells.insert(
            80,
            LightingCell {
                color: Rgb(0),
                effect: LightingEffect::Static,
            },
        );
        let error = configuration.validate(None).unwrap_err().to_string();
        assert!(error.contains("out-of-range key 80"), "{error}");
    }
}
