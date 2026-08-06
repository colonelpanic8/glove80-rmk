//! Bidirectional TOML representation of managed Rynk runtime state.

use std::collections::BTreeMap;

use anyhow::{bail, Context, Result};
use rynk::rmk_types::action::{Action, KeyAction};
use rynk::rmk_types::ble::BleState as WireBleState;
use rynk::rmk_types::combo::Combo;
use rynk::rmk_types::morse::{Morse, MorseMode, MorseProfile};
use rynk::rmk_types::protocol::rynk::{
    LightingActiveTransport, LightingBackgroundMode, LightingBackgroundState,
    LightingBatteryCondition, LightingBondedSlotCondition, LightingChargeCondition,
    LightingConditionSet, LightingConditionalSceneCell, LightingConnectionCondition,
    LightingEffect, LightingEffectsCondition, LightingExtendedConditionalSceneCell,
    LightingExtensionState, LightingLayerCondition, LightingLayerPolicy, LightingLedId,
    LightingNodeId, LightingOutputMode, LightingRgb8, LightingSceneCell,
};
use serde::{Deserialize, Serialize};

pub const ROWS: u8 = 6;
pub const COLS: u8 = 14;
pub const LAYER_SIZE: usize = ROWS as usize * COLS as usize;
pub const HOLES: [usize; 4] = [5, 8, 75, 78];

#[derive(Debug)]
pub struct DiffFound;

impl std::fmt::Display for DiffFound {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("keyboard configuration differs")
    }
}

impl std::error::Error for DiffFound {}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RuntimeConfig {
    #[serde(default)]
    pub default_layer: u8,
    #[serde(default, rename = "layer")]
    pub layers: Vec<LayerConfig>,
    /// The behavior tables the keymap addresses by index. An absent section
    /// means the file says nothing about that table and apply leaves it alone,
    /// so a configuration written before these existed still round-trips.
    #[serde(default, rename = "morse", skip_serializing_if = "Vec::is_empty")]
    pub morses: Vec<MorseConfig>,
    #[serde(default, rename = "combo", skip_serializing_if = "Vec::is_empty")]
    pub combos: Vec<ComboConfig>,
    #[serde(default, rename = "macro", skip_serializing_if = "Vec::is_empty")]
    pub macros: Vec<MacroConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lighting: Option<LightingConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LayerConfig {
    pub id: String,
    pub name: String,
    pub keys: String,
}

/// A morse (tap-dance) key, addressed from the keymap as `TD(n)` by its
/// position in the file.
///
/// Actions are written with the same keycode names `keys` uses. Timing fields
/// left out fall through to the keyboard's global defaults, which is how a
/// thumb key and a home row mod can run different windows.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MorseConfig {
    /// Host-side label; the firmware does not store it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// The wire `Morse` holds each pattern independently, so both of these are
    /// optional: a tap-dance defines `tap` and `double_tap` with no hold at all,
    /// and rendering `hold = ""` for it would produce a file that no longer
    /// parses. At least one action must be present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tap: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub double_tap: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold_after_tap: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold_timeout_ms: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap_timeout_ms: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quick_tap_ms: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prior_idle_ms: Option<u16>,
    /// Send the tap when the interrupting key is on the same hand. This is
    /// what makes a home row mod bilateral, and it needs the firmware's layout
    /// to declare each key's hand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unilateral_tap: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retro_tap: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold_trigger_on_release: Option<bool>,
    /// `normal`, `permissive-hold`, `hold-on-other-press` or
    /// `tap-unless-interrupted`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

/// A combo: pressing every key in `keys` together emits `output`.
///
/// The triggers are actions rather than positions, so a combo follows the keys
/// it names wherever they sit.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ComboConfig {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    pub keys: Vec<String>,
    pub output: String,
    /// Restrict the combo to one layer. Absent means every layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<u8>,
}

/// A macro, addressed from the keymap as `TriggerMacro(n)` by its position in
/// the file. The operation vocabulary is the firmware's own.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MacroConfig {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    pub operations: Vec<MacroOperationConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "operation", rename_all = "lowercase")]
pub enum MacroOperationConfig {
    Tap { keycode: String },
    Down { keycode: String },
    Up { keycode: String },
    Delay { ms: u16 },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LightingConfig {
    pub brightness: u8,
    pub output_mode: OutputModeConfig,
    pub scene_policy: ScenePolicyConfig,
    pub background: BackgroundConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effects: Option<EffectsConfig>,
    #[serde(default, rename = "scene")]
    pub scenes: Vec<SceneConfig>,
    #[serde(default, rename = "conditional_scene")]
    pub conditional_scenes: Vec<ConditionalSceneConfig>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum OutputModeConfig {
    AlwaysOn,
    AlwaysOff,
    PoweredOnly,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ScenePolicyConfig {
    EffectiveOnly,
    ActiveStack,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum BackgroundModeConfig {
    Solid,
    Breathe,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct BackgroundConfig {
    pub enabled: bool,
    pub hue: u8,
    pub saturation: u8,
    pub value: u8,
    pub speed: u8,
    pub mode: BackgroundModeConfig,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct EffectsConfig {
    pub effect: String,
    /// Optional second effect from the same advertised list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overlay: Option<String>,
    pub palette: String,
    pub value: u8,
    pub speed: u8,
    /// Per-effect tunable parameters, keyed by the effect name and then by the
    /// parameter name the firmware advertises:
    ///
    /// ```toml
    /// [lighting.effects.params.Rain]
    /// Density = 6
    /// ```
    ///
    /// A file owns only the parameters it lists. Parameters it omits keep
    /// whatever value the keyboard already holds; they are never reset to
    /// their firmware defaults. `pull` records only parameters that differ
    /// from their default so pulled files stay small.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, BTreeMap<String, u8>>,
}

impl EffectsConfig {
    /// The extension selection without the parameter tables, so parameter
    /// differences are reported per parameter instead of as one opaque blob.
    pub fn selection(&self) -> EffectSelection<'_> {
        EffectSelection {
            effect: &self.effect,
            overlay: self.overlay.as_deref(),
            palette: &self.palette,
            value: self.value,
            speed: self.speed,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct EffectSelection<'a> {
    effect: &'a str,
    overlay: Option<&'a str>,
    palette: &'a str,
    value: u8,
    speed: u8,
}

/// One extension effect's advertised parameters, as read from a keyboard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectParams {
    /// Index of the effect within the advertised effect-name list.
    pub index: u8,
    pub effect: String,
    pub params: Vec<ParamSpec>,
}

/// One parameter's static descriptor plus its live value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParamSpec {
    pub name: String,
    pub min: u8,
    pub max: u8,
    pub default: u8,
    pub value: u8,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum EffectKind {
    Solid,
    Blink,
    Breathe,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct SceneConfig {
    pub layer: u8,
    pub led: u16,
    pub color: String,
    #[serde(default = "solid")]
    pub effect: EffectKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub period_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duty: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_ms: Option<u16>,
}

/// One conditional lighting rule the host owns, as opposed to the ones a board
/// compiles in. A cell applies when every condition it names is satisfied;
/// naming none makes it unconditional.
///
/// Order is meaningful — matching rules compose in table order and later cells
/// win the slots they share — so this list is never sorted, unlike
/// [`SceneConfig`].
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ConditionalSceneConfig {
    pub led: u16,
    pub color: String,
    #[serde(default = "solid")]
    pub effect: EffectKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub period_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duty: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_ms: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<LayerConditionConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub battery: Option<BatteryConditionConfig>,
    /// Gate on the live output-mode policy, which is how the mode indicator is
    /// expressed as an ordinary rule instead of something compiled in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_mode: Option<OutputModeConfig>,
    /// Gate on the live connection: the transport carrying output, the
    /// selected BLE profile, and that profile's state. This is how the
    /// connection-slot indicator is expressed as ordinary rules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection: Option<ConnectionConditionConfig>,
    /// Gate on whether the animated extension band is rendering, which is
    /// what `RGB_TOG` flips. This is how a key bound to that toggle can show
    /// its own state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effects: Option<EffectsConditionConfig>,
}

/// Gate a rule on the extension band being on or off.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct EffectsConditionConfig {
    pub enabled: bool,
}

/// Gate a rule on the keyboard's live connection state. Every named field
/// must hold; `profile` and `ble_state` describe the selected BLE slot
/// whether or not BLE is the active transport.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ConnectionConditionConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<TransportConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ble_state: Option<BleStateConfig>,
    /// Gate on one slot holding a stored bond, whichever profile is active.
    /// `profile` can only ever describe the selected slot, so this is what
    /// lets one rule per slot key say "paired" or "empty" for all of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bonded: Option<BondedSlotConditionConfig>,
    /// Gate on USB being plugged and routable, whether or not it is the
    /// transport actually carrying output. `transport = "usb"` is the
    /// narrower "USB is carrying typing right now"; this is the difference
    /// between a USB key shown ready and one shown active.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usb_connected: Option<bool>,
}

/// Highest addressable BLE profile slot. The board compiles in its profile
/// count, so this is the host-side mirror of that bound and has to move with
/// `ble_profiles_num`.
const MAX_BLE_SLOT: u8 = 3;

/// Gate a rule on one BLE slot's stored bond.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct BondedSlotConditionConfig {
    pub slot: u8,
    pub bonded: bool,
}

/// The transport actually carrying HID output; `none` matches a keyboard
/// that is neither USB-ready nor BLE-connected.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TransportConfig {
    Usb,
    Ble,
    None,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum BleStateConfig {
    Advertising,
    Connected,
    Inactive,
}

/// Gate a rule on a layer being active, or deliberately inactive.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct LayerConditionConfig {
    pub layer: u8,
    #[serde(default = "yes")]
    pub active: bool,
}

/// Gate a rule on one half's battery. Levels are percentages; omitting a bound
/// leaves that side open.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct BatteryConditionConfig {
    pub node: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_level: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_level: Option<u8>,
    #[serde(default, skip_serializing_if = "is_any_charge")]
    pub charge: ChargeConditionConfig,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ChargeConditionConfig {
    #[default]
    Any,
    Charging,
    Discharging,
    Unknown,
}

const fn yes() -> bool {
    true
}

fn is_any_charge(charge: &ChargeConditionConfig) -> bool {
    matches!(charge, ChargeConditionConfig::Any)
}

const fn solid() -> EffectKind {
    EffectKind::Solid
}

/// The comparable form of a whole managed configuration: keycodes resolved to
/// their VIA numbers and lighting canonicalized, so a file and a keyboard can
/// be diffed and applied field by field. Produced either by validating a
/// [`RuntimeConfig`] or by reading a live keyboard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub default_layer: u8,
    pub layers: Vec<Vec<u16>>,
    pub lighting: Option<LightingSnapshot>,
    /// The behavior tables a keymap cell addresses by index: morses for
    /// `TD(n)`, macros for `TriggerMacro(n)`, and the combos that fire
    /// alongside them.
    ///
    /// `None` means the source does not describe the table and it should be
    /// left alone, the way the lighting fields distinguish silence from
    /// emptiness — so a file written before the `[[morse]]`, `[[combo]]` and
    /// `[[macro]]` sections existed can never read as "clear them".
    pub behaviors: BehaviorSnapshot,
}

/// The behavior half of a [`Snapshot`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BehaviorSnapshot {
    pub morses: Option<Vec<rynk::rmk_types::morse::Morse>>,
    pub combos: Option<Vec<rynk::rmk_types::combo::Combo>>,
    /// Macro space as the firmware stores it: the sequences concatenated, each
    /// ended by its own terminator, which is what `TriggerMacro` indexes into.
    pub macros: Option<Vec<u8>>,
}

/// The lighting half of a [`Snapshot`]. Its `Option` fields distinguish state a
/// source is silent about from state it says is empty, which is what keeps
/// older firmware from reading as "delete everything".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LightingSnapshot {
    pub brightness: u8,
    pub output_mode: OutputModeConfig,
    pub scene_policy: ScenePolicyConfig,
    pub background: BackgroundConfig,
    pub effects: Option<EffectsConfig>,
    /// Parameters the keyboard advertises. `None` in a file snapshot, and also
    /// on firmware that does not implement the parameter commands at all.
    pub params: Option<Vec<EffectParams>>,
    pub scenes: Vec<SceneConfig>,
    /// Host-owned conditional rules. `None` on firmware that does not
    /// implement the runtime conditional commands, which keeps "not supported"
    /// distinct from "supported and empty".
    pub conditional_scenes: Option<Vec<ConditionalSceneConfig>>,
}

impl RuntimeConfig {
    /// Deserialize and validate runtime TOML. Validation is exactly what
    /// [`Self::snapshot`] checks, so a config that parses here is one a
    /// keyboard can be asked to hold.
    pub fn from_toml(text: &str) -> Result<Self> {
        let config: Self = toml::from_str(text)?;
        config.snapshot().map(|_| config)
    }

    pub fn snapshot(&self) -> Result<Snapshot> {
        if self.layers.is_empty() {
            bail!("configuration must contain at least one [[layer]]");
        }
        let mut ids = BTreeMap::new();
        let mut layers = Vec::with_capacity(self.layers.len());
        for (index, layer) in self.layers.iter().enumerate() {
            if layer.id.trim().is_empty() || layer.name.trim().is_empty() {
                bail!("layer {index} must have non-empty id and name");
            }
            if ids.insert(&layer.id, index).is_some() {
                bail!("duplicate layer id '{}'", layer.id);
            }
            layers.push(
                parse_keys(&layer.keys)
                    .with_context(|| format!("layer {} ({})", index, layer.id))?,
            );
        }
        if usize::from(self.default_layer) >= layers.len() {
            bail!(
                "default_layer {} is outside the {} configured layers",
                self.default_layer,
                layers.len()
            );
        }
        let lighting = self
            .lighting
            .as_ref()
            .map(LightingConfig::snapshot)
            .transpose()?;
        Ok(Snapshot {
            default_layer: self.default_layer,
            layers,
            lighting,
            behaviors: BehaviorSnapshot {
                morses: (!self.morses.is_empty())
                    .then(|| {
                        self.morses
                            .iter()
                            .enumerate()
                            .map(|(index, morse)| {
                                morse
                                    .to_wire()
                                    .with_context(|| format!("[[morse]] {index} ({})", morse.name))
                            })
                            .collect::<Result<Vec<_>>>()
                    })
                    .transpose()?,
                combos: (!self.combos.is_empty())
                    .then(|| {
                        self.combos
                            .iter()
                            .enumerate()
                            .map(|(index, combo)| {
                                combo
                                    .to_wire()
                                    .with_context(|| format!("[[combo]] {index} ({})", combo.name))
                            })
                            .collect::<Result<Vec<_>>>()
                    })
                    .transpose()?,
                macros: (!self.macros.is_empty())
                    .then(|| {
                        let mut space = Vec::new();
                        for (index, entry) in self.macros.iter().enumerate() {
                            space.extend(
                                entry.to_wire().with_context(|| {
                                    format!("[[macro]] {index} ({})", entry.name)
                                })?,
                            );
                        }
                        Ok::<_, anyhow::Error>(space)
                    })
                    .transpose()?,
            },
        })
    }

    pub fn from_snapshot(snapshot: &Snapshot, labels: Option<&RuntimeConfig>) -> Self {
        let layers = snapshot
            .layers
            .iter()
            .enumerate()
            .map(|(index, keys)| {
                let old = labels.and_then(|config| config.layers.get(index));
                LayerConfig {
                    id: old.map_or_else(|| format!("layer{index}"), |layer| layer.id.clone()),
                    name: old.map_or_else(|| format!("Layer {index}"), |layer| layer.name.clone()),
                    keys: render_keys(keys),
                }
            })
            .collect();
        Self {
            default_layer: snapshot.default_layer,
            layers,
            morses: used_slots(
                snapshot.behaviors.morses.as_deref().unwrap_or_default(),
                |morse| morse.actions.is_empty(),
            )
            .iter()
            .enumerate()
            .map(|(index, morse)| {
                let mut config = MorseConfig::from_wire(morse, index);
                // The firmware stores no label, so keep the one a previous
                // file gave this slot, as layer names are kept.
                if let Some(old) = labels.and_then(|config| config.morses.get(index)) {
                    config.name = old.name.clone();
                }
                config
            })
            .collect(),
            combos: used_slots(
                snapshot.behaviors.combos.as_deref().unwrap_or_default(),
                |combo| combo.actions.is_empty(),
            )
            .iter()
            .enumerate()
            .map(|(index, combo)| {
                let mut config = ComboConfig::from_wire(combo, index);
                if let Some(old) = labels.and_then(|config| config.combos.get(index)) {
                    config.name = old.name.clone();
                }
                config
            })
            .collect(),
            macros: snapshot
                .behaviors
                .macros
                .as_deref()
                .map(MacroConfig::all_from_wire)
                .unwrap_or_default(),
            lighting: snapshot
                .lighting
                .as_ref()
                .map(LightingConfig::from_snapshot),
        }
    }

    pub fn to_toml(&self) -> Result<String> {
        let mut text =
            toml::to_string_pretty(self).context("could not serialize runtime configuration")?;
        if !text.ends_with('\n') {
            text.push('\n');
        }
        Ok(text)
    }
}

/// The part of a behavior table a device read actually describes.
///
/// A read answers with the whole table, because its length is the firmware's
/// compile-time capacity rather than the number of entries in use, and a
/// Glove80 has room for 128 morses and 32 combos. Everything past the last used
/// slot is capacity, not configuration, so it has no business in a file. The
/// tail is all that goes: a keymap cell addresses a morse as `TD(n)` by
/// position, so a gap between two used slots keeps its place.
fn used_slots<T>(table: &[T], empty: impl Fn(&T) -> bool) -> &[T] {
    let used = table.iter().rposition(|entry| !empty(entry));
    &table[..used.map_or(0, |index| index + 1)]
}

/// Spell a wire action the way the keymap's `keys` field spells one.
fn action_name(action: Action) -> String {
    crate::keycodes::format_keycode(crate::rynk_keycode::to_via_keycode(KeyAction::Single(
        action,
    )))
}

fn action_from_name(text: &str) -> Result<Action> {
    match crate::rynk_keycode::from_via_keycode(crate::keycodes::parse_keycode(text)?) {
        KeyAction::Single(action) => Ok(action),
        // `KC_NO` decodes to the empty *key action*, and on a morse pattern it
        // means the same thing the keymap means by it: this holds nothing.
        KeyAction::No => Ok(Action::No),
        _ => bail!("'{text}' is not a single action"),
    }
}

impl MorseConfig {
    pub(crate) fn to_wire(&self) -> Result<Morse> {
        use rynk::rmk_types::morse::{DOUBLE_TAP, HOLD, HOLD_AFTER_TAP, TAP};

        let mode = match self.mode.as_deref() {
            None => None,
            Some("normal") => Some(MorseMode::Normal),
            Some("permissive-hold") => Some(MorseMode::PermissiveHold),
            Some("hold-on-other-press") => Some(MorseMode::HoldOnOtherPress),
            Some("tap-unless-interrupted") => Some(MorseMode::TapUnlessInterrupted),
            Some(other) => bail!("unknown morse mode '{other}'"),
        };
        let profile = MorseProfile::const_default()
            .with_mode(mode)
            .with_hold_timeout_ms(self.hold_timeout_ms)
            .with_gap_timeout_ms(self.gap_timeout_ms)
            .with_quick_tap_timeout_ms(self.quick_tap_ms)
            .with_prior_idle_time_ms(self.prior_idle_ms)
            .with_unilateral_tap(self.unilateral_tap)
            .with_retro_tap(self.retro_tap)
            .with_hold_trigger_on_release(self.hold_trigger_on_release);

        let mut morse = Morse {
            profile,
            ..Morse::default()
        };
        if let Some(text) = &self.tap {
            let _ = morse.put(TAP, action_from_name(text).context("tap action")?);
        }
        if let Some(text) = &self.hold {
            let _ = morse.put(HOLD, action_from_name(text).context("hold action")?);
        }
        if let Some(text) = &self.double_tap {
            let _ = morse.put(DOUBLE_TAP, action_from_name(text).context("double tap")?);
        }
        if let Some(text) = &self.hold_after_tap {
            let _ = morse.put(
                HOLD_AFTER_TAP,
                action_from_name(text).context("hold after tap")?,
            );
        }
        if morse.actions.is_empty() {
            // `KC_NO` is how a file says "nothing here", and a slot the keymap
            // addresses past has to be spellable: dropping it would move every
            // later morse out from under the `TD(n)` that names it. A section
            // that mentions no action at all is still a mistake rather than an
            // empty slot.
            if !self.names_an_action() {
                bail!("a morse needs at least one of tap, hold, double_tap or hold_after_tap");
            }
        }
        Ok(morse)
    }

    /// Whether the file wrote any action for this slot, `KC_NO` included.
    fn names_an_action(&self) -> bool {
        self.tap.is_some()
            || self.hold.is_some()
            || self.double_tap.is_some()
            || self.hold_after_tap.is_some()
    }

    pub(crate) fn from_wire(morse: &Morse, index: usize) -> Self {
        use rynk::rmk_types::morse::{DOUBLE_TAP, HOLD, HOLD_AFTER_TAP, TAP};

        Self {
            name: format!("morse {index}"),
            // An unused slot between two used ones is written out as an
            // explicit `KC_NO` tap, which is what keeps the later slots at the
            // indices the keymap already points at.
            tap: morse
                .get(TAP)
                .map(action_name)
                .or_else(|| morse.actions.is_empty().then(|| action_name(Action::No))),
            hold: morse.get(HOLD).map(action_name),
            double_tap: morse.get(DOUBLE_TAP).map(action_name),
            hold_after_tap: morse.get(HOLD_AFTER_TAP).map(action_name),
            hold_timeout_ms: morse.profile.hold_timeout_ms(),
            gap_timeout_ms: morse.profile.gap_timeout_ms(),
            quick_tap_ms: morse.profile.quick_tap_timeout_ms(),
            prior_idle_ms: morse.profile.prior_idle_time_ms(),
            unilateral_tap: morse.profile.unilateral_tap(),
            retro_tap: morse.profile.retro_tap(),
            hold_trigger_on_release: morse.profile.hold_trigger_on_release(),
            mode: morse.profile.mode().map(|mode| {
                match mode {
                    MorseMode::Normal => "normal",
                    MorseMode::PermissiveHold => "permissive-hold",
                    MorseMode::HoldOnOtherPress => "hold-on-other-press",
                    MorseMode::TapUnlessInterrupted => "tap-unless-interrupted",
                }
                .to_owned()
            }),
        }
    }
}

impl ComboConfig {
    pub(crate) fn to_wire(&self) -> Result<Combo> {
        let mut actions = heapless::Vec::new();
        for key in &self.keys {
            let action =
                crate::rynk_keycode::from_via_keycode(crate::keycodes::parse_keycode(key)?);
            actions
                .push(action)
                .map_err(|_| anyhow::anyhow!("more keys than a combo can hold"))?;
        }
        if actions.len() < 2 {
            // An unused slot in the middle of the table is written out as a
            // combo with no keys and no output, and has to parse back into the
            // same gap rather than be rejected as a half-written rule.
            let empty_slot = actions.is_empty()
                && crate::rynk_keycode::from_via_keycode(crate::keycodes::parse_keycode(
                    &self.output,
                )?) == KeyAction::No;
            if !empty_slot {
                bail!("a combo needs at least two keys");
            }
        }
        Ok(Combo {
            actions,
            output: crate::rynk_keycode::from_via_keycode(crate::keycodes::parse_keycode(
                &self.output,
            )?),
            layer: self.layer,
        })
    }

    pub(crate) fn from_wire(combo: &Combo, index: usize) -> Self {
        Self {
            name: format!("combo {index}"),
            keys: combo
                .actions
                .iter()
                .map(|action| {
                    crate::keycodes::format_keycode(crate::rynk_keycode::to_via_keycode(*action))
                })
                .collect(),
            output: crate::keycodes::format_keycode(crate::rynk_keycode::to_via_keycode(
                combo.output,
            )),
            layer: combo.layer,
        }
    }
}

impl MacroConfig {
    /// One macro's bytes, terminator included.
    pub(crate) fn to_wire(&self) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        for operation in &self.operations {
            let (tag, keycode) = match operation {
                MacroOperationConfig::Tap { keycode } => (0x01, Some(keycode)),
                MacroOperationConfig::Down { keycode } => (0x02, Some(keycode)),
                MacroOperationConfig::Up { keycode } => (0x03, Some(keycode)),
                MacroOperationConfig::Delay { ms } => {
                    // Vial packs a delay as two bytes that are never zero.
                    bytes.extend_from_slice(&[
                        0x01,
                        0x04,
                        (ms % 255) as u8 + 1,
                        (ms / 255) as u8 + 1,
                    ]);
                    continue;
                }
            };
            let Some(keycode) = keycode else { continue };
            let code = crate::keycodes::parse_keycode(keycode)?;
            // A modified keycode has no one-byte form, so the modifiers are
            // pressed around the key instead.
            let modifiers = modifier_hid_keys((code >> 8) as u8);
            for modifier in &modifiers {
                bytes.extend_from_slice(&[0x01, 0x02, *modifier]);
            }
            bytes.extend_from_slice(&[0x01, tag, (code & 0xff) as u8]);
            for modifier in modifiers.iter().rev() {
                bytes.extend_from_slice(&[0x01, 0x03, *modifier]);
            }
        }
        bytes.push(0x00);
        Ok(bytes)
    }

    /// Split macro space back into one entry per terminator.
    pub(crate) fn all_from_wire(space: &[u8]) -> Vec<Self> {
        let mut out = Vec::new();
        for (index, sequence) in space.split(|byte| *byte == 0).enumerate() {
            if sequence.is_empty() {
                continue;
            }
            let mut operations = Vec::new();
            let mut cursor = 0;
            while cursor + 2 < sequence.len() + 1 && cursor + 1 < sequence.len() {
                match (sequence[cursor], sequence.get(cursor + 1)) {
                    (0x01, Some(0x04)) if cursor + 3 < sequence.len() => {
                        let ms = (sequence[cursor + 2].max(1) as u16 - 1)
                            + (sequence[cursor + 3].max(1) as u16 - 1) * 255;
                        operations.push(MacroOperationConfig::Delay { ms });
                        cursor += 4;
                    }
                    (0x01, Some(tag @ 0x01..=0x03)) if cursor + 2 < sequence.len() => {
                        let keycode = crate::keycodes::format_keycode(sequence[cursor + 2] as u16);
                        operations.push(match tag {
                            0x01 => MacroOperationConfig::Tap { keycode },
                            0x02 => MacroOperationConfig::Down { keycode },
                            _ => MacroOperationConfig::Up { keycode },
                        });
                        cursor += 3;
                    }
                    _ => break,
                }
            }
            out.push(Self {
                name: format!("macro {index}"),
                operations,
            });
        }
        out
    }
}

/// The HID keycodes for a VIA packed-modifier byte, in a stable order.
fn modifier_hid_keys(packed: u8) -> Vec<u8> {
    const MODIFIERS: [(u8, u8); 4] = [
        (0b0000_0001, 0xe0), // Ctrl
        (0b0000_0010, 0xe1), // Shift
        (0b0000_0100, 0xe2), // Alt
        (0b0000_1000, 0xe3), // Gui
    ];
    // Bit 4 selects the right-hand set, which sits four keycodes further on.
    let right = if packed & 0b0001_0000 != 0 { 4 } else { 0 };
    MODIFIERS
        .iter()
        .filter(|(bit, _)| packed & bit != 0)
        .map(|(_, key)| key + right)
        .collect()
}

impl LightingConfig {
    pub fn snapshot(&self) -> Result<LightingSnapshot> {
        let mut conditional_scenes = self.conditional_scenes.clone();
        for (index, cell) in conditional_scenes.iter_mut().enumerate() {
            cell.color = normalize_color(&cell.color)?;
            validate_conditional_scene(index, cell)?;
        }
        let mut scenes = self.scenes.clone();
        for cell in &mut scenes {
            cell.color = normalize_color(&cell.color)?;
            validate_scene(cell)?;
        }
        scenes.sort();
        let duplicate = scenes
            .windows(2)
            .find(|pair| pair[0].layer == pair[1].layer && pair[0].led == pair[1].led);
        if let Some(pair) = duplicate {
            bail!(
                "duplicate scene cell for layer {} LED {}",
                pair[0].layer,
                pair[0].led
            );
        }
        if let Some(effects) = &self.effects {
            for (effect, table) in &effects.params {
                if effect.trim().is_empty() {
                    bail!("[lighting.effects.params] has an empty effect name");
                }
                if table.keys().any(|name| name.trim().is_empty()) {
                    bail!("effect '{effect}' has an empty parameter name");
                }
            }
        }
        Ok(LightingSnapshot {
            brightness: self.brightness,
            output_mode: self.output_mode,
            scene_policy: self.scene_policy,
            background: self.background.clone(),
            effects: self.effects.clone(),
            params: None,
            scenes,
            conditional_scenes: Some(conditional_scenes),
        })
    }

    pub fn from_snapshot(snapshot: &LightingSnapshot) -> Self {
        Self {
            brightness: snapshot.brightness,
            output_mode: snapshot.output_mode,
            scene_policy: snapshot.scene_policy,
            background: snapshot.background.clone(),
            effects: snapshot.effects.clone(),
            scenes: snapshot.scenes.clone(),
            conditional_scenes: snapshot.conditional_scenes.clone().unwrap_or_default(),
        }
    }
}

impl RuntimeConfig {
    /// Drop parameters that still hold their firmware default, so a pulled
    /// file records only what the user actually tuned.
    pub fn retain_non_default_params(&mut self, snapshot: &Snapshot) {
        let Some(effects) = self
            .lighting
            .as_mut()
            .and_then(|lighting| lighting.effects.as_mut())
        else {
            return;
        };
        let advertised = snapshot
            .lighting
            .as_ref()
            .and_then(|lighting| lighting.params.as_ref());
        let Some(advertised) = advertised else {
            effects.params.clear();
            return;
        };
        for set in advertised {
            let Some(table) = effects.params.get_mut(&set.effect) else {
                continue;
            };
            table.retain(|name, value| {
                set.params
                    .iter()
                    .find(|spec| spec.name == *name)
                    .is_none_or(|spec| spec.default != *value)
            });
        }
        effects.params.retain(|_, table| !table.is_empty());
    }
}

/// Render live parameter values as the schema's `[lighting.effects.params.…]`
/// tables. `show` prints these as-is; `pull` prunes defaults from them first.
pub fn live_param_tables(sets: Option<&[EffectParams]>) -> BTreeMap<String, BTreeMap<String, u8>> {
    sets.unwrap_or_default()
        .iter()
        .map(|set| {
            let table = set
                .params
                .iter()
                .map(|param| (param.name.clone(), param.value))
                .collect();
            (set.effect.clone(), table)
        })
        .collect()
}

/// Report every parameter a file lists whose value the keyboard does not
/// already hold. Parameters absent from the file are not compared: a file owns
/// only what it names.
pub fn param_differences(desired: &LightingSnapshot, live: &LightingSnapshot) -> Vec<String> {
    let mut result = Vec::new();
    let Some(wanted) = desired.effects.as_ref() else {
        return result;
    };
    for (effect, table) in &wanted.params {
        let set = live
            .params
            .as_deref()
            .unwrap_or_default()
            .iter()
            .find(|set| set.effect == *effect);
        for (name, value) in table {
            let current = set.and_then(|set| set.params.iter().find(|param| param.name == *name));
            match current {
                Some(param) if param.value == *value => {}
                Some(param) => result.push(format!(
                    "lighting parameter {effect}.{name}: file {value} != keyboard {}",
                    param.value
                )),
                None => result.push(format!(
                    "lighting parameter {effect}.{name}: file {value} != keyboard (not advertised)"
                )),
            }
        }
    }
    result
}

pub fn differences(desired: &Snapshot, live: &Snapshot) -> Vec<String> {
    let mut result = Vec::new();
    if desired.default_layer != live.default_layer {
        result.push(format!(
            "default layer: file {} != keyboard {}",
            desired.default_layer, live.default_layer
        ));
    }
    for layer in 0..desired.layers.len() {
        for offset in 0..LAYER_SIZE {
            let wanted = desired.layers.get(layer).map_or(0, |keys| keys[offset]);
            let present = live.layers.get(layer).map_or(0, |keys| keys[offset]);
            if wanted != present {
                result.push(format!(
                    "layer {layer} r{},c{}: file {} != keyboard {}",
                    offset / usize::from(COLS),
                    offset % usize::from(COLS),
                    crate::keycodes::format_keycode(wanted),
                    crate::keycodes::format_keycode(present),
                ));
            }
        }
    }
    // A table the source is silent about is left alone, so only a `Some`
    // participates in the diff.
    if let Some(wanted) = &desired.behaviors.morses {
        let present = live.behaviors.morses.as_deref().unwrap_or_default();
        for (index, morse) in wanted.iter().enumerate() {
            if present.get(index) != Some(morse) {
                result.push(format!("morse {index}: file differs from keyboard"));
            }
        }
    }
    if let Some(wanted) = &desired.behaviors.combos {
        let present = live.behaviors.combos.as_deref().unwrap_or_default();
        for (index, combo) in wanted.iter().enumerate() {
            if present.get(index) != Some(combo) {
                result.push(format!("combo {index}: file differs from keyboard"));
            }
        }
    }
    if let Some(wanted) = &desired.behaviors.macros {
        let present = live.behaviors.macros.as_deref().unwrap_or_default();
        // Macro space is zero-filled past the end, so compare only as far as
        // the file describes.
        if present.len() < wanted.len() || present[..wanted.len()] != wanted[..] {
            result.push(format!("macro space: {} byte(s) differ", wanted.len()));
        }
    }

    match (&desired.lighting, &live.lighting) {
        (Some(wanted), Some(present)) => {
            if wanted.brightness != present.brightness {
                result.push(format!(
                    "lighting brightness: file {} != keyboard {}",
                    wanted.brightness, present.brightness
                ));
            }
            if wanted.output_mode != present.output_mode {
                result.push(format!(
                    "lighting output mode: file {:?} != keyboard {:?}",
                    wanted.output_mode, present.output_mode
                ));
            }
            if wanted.scene_policy != present.scene_policy {
                result.push(format!(
                    "lighting scene policy: file {:?} != keyboard {:?}",
                    wanted.scene_policy, present.scene_policy
                ));
            }
            if wanted.background != present.background {
                result.push("lighting background differs".into());
            }
            let wanted_selection = wanted.effects.as_ref().map(EffectsConfig::selection);
            let present_selection = present.effects.as_ref().map(EffectsConfig::selection);
            if wanted_selection != present_selection {
                result.push(format!(
                    "effects state: file {wanted_selection:?} != keyboard {present_selection:?}"
                ));
            }
            result.extend(param_differences(wanted, present));
            let wanted_cells = wanted
                .scenes
                .iter()
                .map(|cell| ((cell.layer, cell.led), cell))
                .collect::<BTreeMap<_, _>>();
            let present_cells = present
                .scenes
                .iter()
                .map(|cell| ((cell.layer, cell.led), cell))
                .collect::<BTreeMap<_, _>>();
            for key in wanted_cells.keys().chain(present_cells.keys()) {
                if wanted_cells.get(key) != present_cells.get(key) {
                    result.push(format!(
                        "lighting scene layer {} LED {}: file {:?} != keyboard {:?}",
                        key.0,
                        key.1,
                        wanted_cells.get(key),
                        present_cells.get(key),
                    ));
                }
            }
            result.sort();
            result.dedup();

            // Reported by position, and appended after the sort, because the
            // table's order is part of its meaning: two rules that swap places
            // are a real difference even though the set is unchanged.
            if let Some(wanted_conditional) = wanted.conditional_scenes.as_ref() {
                match present.conditional_scenes.as_ref() {
                    None if wanted_conditional.is_empty() => {}
                    None => result.push(format!(
                        "lighting conditional rules: file has {} but the keyboard exposes no runtime conditional table",
                        wanted_conditional.len()
                    )),
                    Some(live) => {
                        for index in 0..wanted_conditional.len().max(live.len()) {
                            let (file, keyboard) = (wanted_conditional.get(index), live.get(index));
                            if file != keyboard {
                                result.push(format!(
                                    "lighting conditional rule {index}: file {file:?} != keyboard {keyboard:?}"
                                ));
                            }
                        }
                    }
                }
            }
        }
        (Some(_), None) => result.push("file configures lighting but keyboard exposes none".into()),
        (None, _) => {}
    }
    result
}

pub fn parse_keys(text: &str) -> Result<Vec<u16>> {
    let rows = text
        .lines()
        .map(|line| line.split('#').next().unwrap_or_default().trim())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if rows.len() != usize::from(ROWS) {
        bail!(
            "keys must contain {ROWS} non-empty rows, found {}",
            rows.len()
        );
    }
    let mut result = Vec::with_capacity(LAYER_SIZE);
    for (row, line) in rows.iter().enumerate() {
        let tokens = line.split_whitespace().collect::<Vec<_>>();
        if tokens.len() != usize::from(COLS) {
            bail!("row {row} must contain {COLS} keys, found {}", tokens.len());
        }
        for token in tokens {
            result.push(if token == "--" {
                0
            } else {
                crate::keycodes::parse_keycode(token)?
            });
        }
    }
    for hole in HOLES {
        if result[hole] != 0 {
            bail!(
                "physical hole r{},c{} must be --",
                hole / usize::from(COLS),
                hole % usize::from(COLS)
            );
        }
    }
    Ok(result)
}

pub fn render_keys(keys: &[u16]) -> String {
    let mut text = String::from("\n");
    for row in 0..usize::from(ROWS) {
        for col in 0..usize::from(COLS) {
            if col > 0 {
                text.push(' ');
            }
            let offset = row * usize::from(COLS) + col;
            if keys[offset] == 0 {
                text.push_str("--");
            } else {
                // The grid format is whitespace-delimited, so keep composite
                // keycodes as a single token even when the human formatter
                // normally inserts a space after a comma.
                text.push_str(&crate::keycodes::format_keycode(keys[offset]).replace(", ", ","));
            }
        }
        text.push('\n');
    }
    text
}

pub fn action_to_code(action: KeyAction, layer: usize, offset: usize) -> Result<u16> {
    let code = crate::rynk_keycode::to_via_keycode(action);
    if code == 0 && !matches!(action, KeyAction::No) {
        bail!(
            "action {action:?} at layer {layer} r{},c{} cannot be represented in runtime TOML",
            offset / usize::from(COLS),
            offset % usize::from(COLS)
        );
    }
    Ok(code)
}

pub fn normalize_color(text: &str) -> Result<String> {
    let (r, g, b) = crate::color::parse_color(text)?;
    Ok(format!("#{r:02x}{g:02x}{b:02x}"))
}

/// Structural checks only, mirroring [`validate_scene`] and adding the battery
/// bounds the firmware would otherwise reject on apply. Nothing here contacts
/// the keyboard, so `config validate` stays usable offline.
pub fn validate_conditional_scene(index: usize, cell: &ConditionalSceneConfig) -> Result<()> {
    let timings_set = cell.period_ms.is_some()
        || cell.phase_ms.is_some()
        || cell.duty.is_some()
        || cell.step_ms.is_some();
    match cell.effect {
        EffectKind::Solid if timings_set => {
            bail!(
                "solid conditional rule {index} (LED {}) has timing options",
                cell.led
            )
        }
        EffectKind::Solid => {}
        EffectKind::Blink => {
            if cell.period_ms.unwrap_or(0) == 0 || cell.duty.unwrap_or(101) > 100 {
                bail!(
                    "blink conditional rule {index} (LED {}) needs a non-zero period_ms and a duty of 0..=100",
                    cell.led
                );
            }
        }
        EffectKind::Breathe => {
            if cell.period_ms.unwrap_or(0) == 0 || cell.step_ms.unwrap_or(0) == 0 {
                bail!(
                    "breathe conditional rule {index} (LED {}) needs a non-zero period_ms and step_ms",
                    cell.led
                );
            }
        }
    }
    if let Some(battery) = cell.battery {
        let over = |level: Option<u8>| level.is_some_and(|value| value > 100);
        if over(battery.min_level) || over(battery.max_level) {
            bail!(
                "conditional rule {index} (LED {}) has a battery level above 100",
                cell.led
            );
        }
        if matches!((battery.min_level, battery.max_level), (Some(min), Some(max)) if min > max) {
            let (min, max) = (battery.min_level.unwrap(), battery.max_level.unwrap());
            bail!(
                "conditional rule {index} (LED {}) has battery min_level {min} above max_level {max}",
                cell.led
            );
        }
    }
    if let Some(connection) = cell.connection {
        if connection.transport.is_none()
            && connection.profile.is_none()
            && connection.ble_state.is_none()
            && connection.bonded.is_none()
            && connection.usb_connected.is_none()
        {
            bail!(
                "conditional rule {index} (LED {}) has a connection condition that names no gate",
                cell.led
            );
        }
        // Both bounds describe the same slot space, so they move together when
        // the board's profile count changes.
        if connection
            .profile
            .is_some_and(|profile| profile > MAX_BLE_SLOT)
        {
            bail!(
                "conditional rule {index} (LED {}) names a BLE profile past the board's slots (0-{MAX_BLE_SLOT})",
                cell.led
            );
        }
        if connection
            .bonded
            .is_some_and(|bonded| bonded.slot > MAX_BLE_SLOT)
        {
            bail!(
                "conditional rule {index} (LED {}) names a bonded slot past the board's slots (0-{MAX_BLE_SLOT})",
                cell.led
            );
        }
    }
    Ok(())
}

pub fn validate_scene(cell: &SceneConfig) -> Result<()> {
    match cell.effect {
        EffectKind::Solid => {
            if cell.period_ms.is_some()
                || cell.phase_ms.is_some()
                || cell.duty.is_some()
                || cell.step_ms.is_some()
            {
                bail!(
                    "solid scene layer {} LED {} has timing options",
                    cell.layer,
                    cell.led
                );
            }
        }
        EffectKind::Blink => {
            if cell.period_ms.unwrap_or(0) == 0
                || cell.duty.unwrap_or(101) > 100
                || cell.step_ms.is_some()
            {
                bail!(
                    "invalid blink scene at layer {} LED {}",
                    cell.layer,
                    cell.led
                );
            }
        }
        EffectKind::Breathe => {
            if cell.period_ms.unwrap_or(0) < 2
                || cell.step_ms.unwrap_or(0) == 0
                || cell.duty.is_some()
            {
                bail!(
                    "invalid breathe scene at layer {} LED {}",
                    cell.layer,
                    cell.led
                );
            }
        }
    }
    Ok(())
}

/// Split a wire effect into the flat fields both scene tables use.
pub fn effect_from_wire(
    effect: LightingEffect,
) -> (
    LightingRgb8,
    EffectKind,
    Option<u32>,
    Option<u32>,
    Option<u8>,
    Option<u16>,
) {
    match effect {
        LightingEffect::Solid { color } => (color, EffectKind::Solid, None, None, None, None),
        LightingEffect::Blink {
            color,
            period_ms,
            phase_ms,
            duty,
        } => (
            color,
            EffectKind::Blink,
            Some(period_ms),
            Some(phase_ms),
            Some(duty),
            None,
        ),
        LightingEffect::Breathe {
            color,
            period_ms,
            phase_ms,
            step_ms,
        } => (
            color,
            EffectKind::Breathe,
            Some(period_ms),
            Some(phase_ms),
            None,
            Some(step_ms),
        ),
    }
}

pub fn scene_from_wire(cell: LightingSceneCell) -> SceneConfig {
    let (color, effect, period_ms, phase_ms, duty, step_ms) = effect_from_wire(cell.effect);
    SceneConfig {
        layer: cell.layer,
        led: cell.led_id.0,
        color: format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b),
        effect,
        period_ms,
        phase_ms,
        duty,
        step_ms,
    }
}

pub fn conditional_scene_from_wire(
    extended: LightingExtendedConditionalSceneCell,
) -> ConditionalSceneConfig {
    let connection = extended.connection.map(|c| ConnectionConditionConfig {
        transport: c.transport.map(|transport| match transport {
            LightingActiveTransport::Usb => TransportConfig::Usb,
            LightingActiveTransport::Ble => TransportConfig::Ble,
            LightingActiveTransport::NoneActive => TransportConfig::None,
        }),
        profile: c.profile,
        ble_state: c.ble_state.map(|state| match state {
            WireBleState::Advertising => BleStateConfig::Advertising,
            WireBleState::Connected => BleStateConfig::Connected,
            WireBleState::Inactive => BleStateConfig::Inactive,
        }),
        bonded: c.bonded.map(|bonded| BondedSlotConditionConfig {
            slot: bonded.slot,
            bonded: bonded.bonded,
        }),
        usb_connected: c.usb_connected,
    });
    let effects = extended
        .effects
        .map(|c| EffectsConditionConfig { enabled: c.enabled });
    let cell = extended.cell;
    let (color, effect, period_ms, phase_ms, duty, step_ms) = effect_from_wire(cell.effect);
    ConditionalSceneConfig {
        connection,
        effects,
        led: cell.led_id.0,
        color: format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b),
        effect,
        period_ms,
        phase_ms,
        duty,
        step_ms,
        output_mode: cell.conditions.output_mode.map(output_mode_from_wire),
        layer: cell.conditions.layer.map(|c| LayerConditionConfig {
            layer: c.layer,
            active: c.active,
        }),
        battery: cell.conditions.battery.map(|c| BatteryConditionConfig {
            node: c.node.0,
            min_level: c.min_level,
            max_level: c.max_level,
            charge: match c.charge {
                LightingChargeCondition::Any => ChargeConditionConfig::Any,
                LightingChargeCondition::Charging => ChargeConditionConfig::Charging,
                LightingChargeCondition::Discharging => ChargeConditionConfig::Discharging,
                LightingChargeCondition::Unknown => ChargeConditionConfig::Unknown,
            },
        }),
    }
}

/// Build a wire effect from the flat fields both scene tables use.
pub fn effect_to_wire(
    color: &str,
    kind: EffectKind,
    period_ms: Option<u32>,
    phase_ms: Option<u32>,
    duty: Option<u8>,
    step_ms: Option<u16>,
) -> Result<LightingEffect> {
    let (r, g, b) = crate::color::parse_color(color)?;
    let color = LightingRgb8 { r, g, b };
    Ok(match kind {
        EffectKind::Solid => LightingEffect::Solid { color },
        EffectKind::Blink => LightingEffect::Blink {
            color,
            period_ms: period_ms.context("blink period_ms is required")?,
            phase_ms: phase_ms.unwrap_or(0),
            duty: duty.context("blink duty is required")?,
        },
        EffectKind::Breathe => LightingEffect::Breathe {
            color,
            period_ms: period_ms.context("breathe period_ms is required")?,
            phase_ms: phase_ms.unwrap_or(0),
            step_ms: step_ms.context("breathe step_ms is required")?,
        },
    })
}

pub fn scene_to_wire(cell: &SceneConfig) -> Result<LightingSceneCell> {
    Ok(LightingSceneCell {
        layer: cell.layer,
        led_id: LightingLedId(cell.led),
        effect: effect_to_wire(
            &cell.color,
            cell.effect,
            cell.period_ms,
            cell.phase_ms,
            cell.duty,
            cell.step_ms,
        )?,
    })
}

pub fn conditional_scene_to_wire(
    cell: &ConditionalSceneConfig,
) -> Result<LightingExtendedConditionalSceneCell> {
    let connection = cell.connection.map(|c| LightingConnectionCondition {
        transport: c.transport.map(|transport| match transport {
            TransportConfig::Usb => LightingActiveTransport::Usb,
            TransportConfig::Ble => LightingActiveTransport::Ble,
            TransportConfig::None => LightingActiveTransport::NoneActive,
        }),
        profile: c.profile,
        ble_state: c.ble_state.map(|state| match state {
            BleStateConfig::Advertising => WireBleState::Advertising,
            BleStateConfig::Connected => WireBleState::Connected,
            BleStateConfig::Inactive => WireBleState::Inactive,
        }),
        bonded: c.bonded.map(|bonded| LightingBondedSlotCondition {
            slot: bonded.slot,
            bonded: bonded.bonded,
        }),
        usb_connected: c.usb_connected,
    });
    let base = LightingConditionalSceneCell {
        conditions: LightingConditionSet {
            output_mode: cell.output_mode.map(output_mode_to_wire),
            layer: cell.layer.map(|c| LightingLayerCondition {
                layer: c.layer,
                active: c.active,
            }),
            battery: cell.battery.map(|c| LightingBatteryCondition {
                node: LightingNodeId(c.node),
                min_level: c.min_level,
                max_level: c.max_level,
                charge: match c.charge {
                    ChargeConditionConfig::Any => LightingChargeCondition::Any,
                    ChargeConditionConfig::Charging => LightingChargeCondition::Charging,
                    ChargeConditionConfig::Discharging => LightingChargeCondition::Discharging,
                    ChargeConditionConfig::Unknown => LightingChargeCondition::Unknown,
                },
            }),
        },
        led_id: LightingLedId(cell.led),
        effect: effect_to_wire(
            &cell.color,
            cell.effect,
            cell.period_ms,
            cell.phase_ms,
            cell.duty,
            cell.step_ms,
        )?,
    };
    Ok(LightingExtendedConditionalSceneCell {
        cell: base,
        connection,
        effects: cell
            .effects
            .map(|c| LightingEffectsCondition { enabled: c.enabled }),
    })
}

pub fn background_from_wire(state: LightingBackgroundState) -> BackgroundConfig {
    BackgroundConfig {
        enabled: state.enabled,
        hue: state.hue,
        saturation: state.saturation,
        value: state.value,
        speed: state.speed,
        mode: match state.mode {
            LightingBackgroundMode::Solid => BackgroundModeConfig::Solid,
            LightingBackgroundMode::Breathe => BackgroundModeConfig::Breathe,
        },
    }
}

pub fn background_to_wire(state: &BackgroundConfig) -> LightingBackgroundState {
    LightingBackgroundState {
        enabled: state.enabled,
        hue: state.hue,
        saturation: state.saturation,
        value: state.value,
        speed: state.speed,
        mode: match state.mode {
            BackgroundModeConfig::Solid => LightingBackgroundMode::Solid,
            BackgroundModeConfig::Breathe => LightingBackgroundMode::Breathe,
        },
    }
}

pub fn output_mode_from_wire(mode: LightingOutputMode) -> OutputModeConfig {
    match mode {
        LightingOutputMode::AlwaysOn => OutputModeConfig::AlwaysOn,
        LightingOutputMode::AlwaysOff => OutputModeConfig::AlwaysOff,
        LightingOutputMode::PoweredOnly => OutputModeConfig::PoweredOnly,
    }
}

pub fn output_mode_to_wire(mode: OutputModeConfig) -> LightingOutputMode {
    match mode {
        OutputModeConfig::AlwaysOn => LightingOutputMode::AlwaysOn,
        OutputModeConfig::AlwaysOff => LightingOutputMode::AlwaysOff,
        OutputModeConfig::PoweredOnly => LightingOutputMode::PoweredOnly,
    }
}

pub fn scene_policy_from_wire(policy: LightingLayerPolicy) -> ScenePolicyConfig {
    match policy {
        LightingLayerPolicy::EffectiveOnly => ScenePolicyConfig::EffectiveOnly,
        LightingLayerPolicy::ActiveStack => ScenePolicyConfig::ActiveStack,
    }
}

pub fn scene_policy_to_wire(policy: ScenePolicyConfig) -> LightingLayerPolicy {
    match policy {
        ScenePolicyConfig::EffectiveOnly => LightingLayerPolicy::EffectiveOnly,
        ScenePolicyConfig::ActiveStack => LightingLayerPolicy::ActiveStack,
    }
}

/// RMK gives every layer slot its fixed capacity and initializes the unused
/// ones as transparent, so a keyboard always reports more layers than a source
/// file names. Dropping the trailing layers that bind nothing is what lets a
/// five-layer file round-trip against eight-layer firmware.
pub fn trim_trailing_transparent_layers(layers: &mut Vec<Vec<u16>>) {
    while layers.len() > 1
        && layers
            .last()
            .is_some_and(|layer| layer.iter().all(|code| matches!(*code, 0 | 1)))
    {
        layers.pop();
    }
}

/// The file speaks in effect and palette *names*; the protocol speaks in
/// indices into the lists a keyboard advertises. These two functions are the
/// only place that translation lives, so the CLI and the browser cannot drift.
pub fn effects_from_wire(
    state: LightingExtensionState,
    overlay: Option<u8>,
    effect_names: &[String],
    palette_names: &[String],
    params: BTreeMap<String, BTreeMap<String, u8>>,
) -> Result<EffectsConfig> {
    let name = |names: &[String], index: u8, what: &str| {
        names
            .get(usize::from(index))
            .cloned()
            .with_context(|| format!("extension {what} index is outside its advertised name list"))
    };
    Ok(EffectsConfig {
        effect: name(effect_names, state.effect, "effect")?,
        overlay: overlay
            .map(|index| name(effect_names, index, "overlay"))
            .transpose()?,
        palette: name(palette_names, state.palette, "palette")?,
        value: state.value,
        speed: state.speed,
        params,
    })
}

pub fn effects_to_wire(
    effects: &EffectsConfig,
    effect_names: &[String],
    palette_names: &[String],
) -> Result<(LightingExtensionState, Option<u8>)> {
    // An unknown name is the most common way a file and a keyboard disagree,
    // so the message lists what the keyboard does advertise rather than
    // leaving the user to go read it out of `lighting extension`.
    let index = |names: &[String], wanted: &str, what: &str| -> Result<u8> {
        let found = names
            .iter()
            .position(|name| name == wanted)
            .with_context(|| {
                format!(
                    "unknown extension {what} '{wanted}'; the keyboard advertises: {}",
                    names.join(", ")
                )
            })?;
        u8::try_from(found).with_context(|| format!("{what} index exceeds u8"))
    };
    Ok((
        LightingExtensionState {
            effect: index(effect_names, &effects.effect, "effect")?,
            palette: index(palette_names, &effects.palette, "palette")?,
            value: effects.value,
            speed: effects.speed,
        },
        effects
            .overlay
            .as_deref()
            .map(|overlay| index(effect_names, overlay, "overlay effect"))
            .transpose()?,
    ))
}

/// One parameter write addressed the way the protocol addresses it: by the
/// effect's index in the advertised effect list and the parameter's ordinal
/// within that effect's own list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParamWrite {
    pub effect: u8,
    pub index: u8,
    pub value: u8,
    /// What the keyboard currently holds, so a caller can skip no-op writes.
    pub current: u8,
    /// `Effect.Parameter` as the file spells it, for error context.
    pub label: String,
}

/// Resolve the parameters a file names against what the keyboard advertises.
/// Bounds are checked here rather than left to the firmware so a host reports
/// the offending name instead of a bare protocol rejection.
pub fn params_to_writes(
    wanted: &BTreeMap<String, BTreeMap<String, u8>>,
    advertised: Option<&[EffectParams]>,
) -> Result<Vec<ParamWrite>> {
    let advertised =
        advertised.context("the keyboard does not expose per-effect extension parameters")?;
    let mut writes = Vec::new();
    for (effect, table) in wanted {
        let set = advertised
            .iter()
            .find(|set| set.effect == *effect)
            .with_context(|| format!("effect '{effect}' advertises no parameters"))?;
        for (name, value) in table {
            let index = set
                .params
                .iter()
                .position(|param| param.name == *name)
                .with_context(|| format!("effect '{effect}' has no parameter '{name}'"))?;
            let param = &set.params[index];
            if *value < param.min || *value > param.max {
                bail!(
                    "parameter '{effect}.{name}' accepts {}..={}, file requests {value}",
                    param.min,
                    param.max
                );
            }
            writes.push(ParamWrite {
                effect: set.index,
                index: u8::try_from(index).context("parameter index exceeds u8")?,
                value: *value,
                current: param.value,
                label: format!("{effect}.{name}"),
            });
        }
    }
    Ok(writes)
}

/// The inverse of [`params_to_writes`]: name the ordinals a host holds so they
/// can be written back out as `[lighting.effects.params.…]` tables.
pub fn param_tables_from_wire(
    writes: &[(u8, u8, u8)],
    advertised: &[EffectParams],
) -> Result<BTreeMap<String, BTreeMap<String, u8>>> {
    let mut tables: BTreeMap<String, BTreeMap<String, u8>> = BTreeMap::new();
    for (effect, index, value) in writes.iter().copied() {
        let set = advertised
            .iter()
            .find(|set| set.index == effect)
            .with_context(|| format!("effect index {effect} advertises no parameters"))?;
        let param = set
            .params
            .get(usize::from(index))
            .with_context(|| format!("effect '{}' has no parameter {index}", set.effect))?;
        tables
            .entry(set.effect.clone())
            .or_default()
            .insert(param.name.clone(), value);
    }
    Ok(tables)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_style_keymap_round_trips() {
        let keys = "\n-- -- KC_A KC_TRNS LT(1,KC_ESC) -- -- -- -- -- -- -- -- --\n-- -- -- -- -- -- -- -- -- -- -- -- -- --\n-- -- -- -- -- -- -- -- -- -- -- -- -- --\n-- -- -- -- -- -- -- -- -- -- -- -- -- --\n-- -- -- -- -- -- -- -- -- -- -- -- -- --\n-- -- -- -- -- -- -- -- -- -- -- -- -- --\n";
        let parsed = parse_keys(keys).unwrap();
        assert_eq!(parsed[2], 0x0004);
        assert_eq!(parsed[3], 0x0001);
        assert_eq!(parsed[4], 0x4129);
        assert_eq!(parse_keys(&render_keys(&parsed)).unwrap(), parsed);
    }

    #[test]
    fn scene_colors_are_canonicalized() {
        assert_eq!(normalize_color("C000C0").unwrap(), "#c000c0");
    }

    const LIGHTING_WITH_PARAMS: &str = r#"
brightness = 100
output_mode = "always-on"
scene_policy = "effective-only"

[background]
enabled = false
hue = 0
saturation = 0
value = 0
speed = 0
mode = "solid"

[effects]
effect = "Rain"
overlay = "Reactive"
palette = "Aurora"
value = 200
speed = 40

[effects.params.Rain]
Density = 6
"Trail Length" = 128
"#;

    fn params_of(config: &LightingConfig, effect: &str, name: &str) -> Option<u8> {
        config
            .effects
            .as_ref()?
            .params
            .get(effect)?
            .get(name)
            .copied()
    }

    fn param_set(effect: &str, params: &[(&str, u8, u8, u8, u8)]) -> EffectParams {
        EffectParams {
            index: 1,
            effect: effect.to_owned(),
            params: params
                .iter()
                .map(|(name, min, max, default, value)| ParamSpec {
                    name: (*name).to_owned(),
                    min: *min,
                    max: *max,
                    default: *default,
                    value: *value,
                })
                .collect(),
        }
    }

    /// The table's order is part of its meaning, so a reordering has to read as
    /// a difference even though the set of rules is identical.
    #[test]
    fn reordered_conditional_rules_are_a_difference() {
        let rule = |led: u16| ConditionalSceneConfig {
            connection: None,
            led,
            color: "#0040a0".into(),
            effect: EffectKind::Solid,
            period_ms: None,
            phase_ms: None,
            duty: None,
            step_ms: None,
            layer: Some(LayerConditionConfig {
                layer: 2,
                active: true,
            }),
            battery: None,
            output_mode: None,
            effects: None,
        };
        let snapshot = |cells: Vec<ConditionalSceneConfig>| {
            let mut snap = lighting_snapshot(None, None);
            snap.conditional_scenes = Some(cells);
            Snapshot {
                behaviors: BehaviorSnapshot::default(),
                default_layer: 0,
                layers: Vec::new(),
                lighting: Some(snap),
            }
        };

        let forward = snapshot(vec![rule(10), rule(20)]);
        assert!(differences(&forward, &forward).is_empty());

        let reversed = snapshot(vec![rule(20), rule(10)]);
        let found = differences(&forward, &reversed);
        assert_eq!(found.len(), 2, "both positions differ: {found:?}");
        assert!(found.iter().all(|line| line.contains("conditional rule")));
    }

    /// The smallest keymap the parser accepts, with two behavior tables on it.
    fn two_behavior_tables() -> String {
        let row = |holes: bool| {
            (0..14)
                .map(|col| {
                    if holes && (col == 5 || col == 8) {
                        "--"
                    } else {
                        "KC_A"
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        };
        let keys = (0..6)
            .map(|index| row(index == 0 || index == 5))
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "default_layer = 0\n\
             [[layer]]\nid = \"base\"\nname = \"Base\"\nkeys = \"\"\"\n{keys}\n\"\"\"\n\
             [[morse]]\ntap = \"KC_A\"\nhold = \"KC_LSFT\"\n\
             [[combo]]\nkeys = [\"KC_A\", \"KC_B\"]\noutput = \"KC_C\"\n"
        )
    }

    /// A pull reads the whole behavior table, because its size is the
    /// firmware's capacity rather than the number of entries in use. The unused
    /// tail is not configuration: writing it out produced `[[morse]]` sections
    /// that name no action at all, and the file the pull had just written was
    /// then rejected by its own parser.
    #[test]
    fn a_pull_leaves_the_unused_tail_of_a_behavior_table_out_of_the_file() {
        let config = RuntimeConfig::from_toml(&two_behavior_tables()).expect("parse");
        let mut snapshot = config.snapshot().expect("snapshot");
        // What the device answers: the tables padded out to their capacity.
        snapshot
            .behaviors
            .morses
            .as_mut()
            .expect("morses")
            .resize(128, Morse::default());
        snapshot
            .behaviors
            .combos
            .as_mut()
            .expect("combos")
            .resize(32, Combo::empty());

        let pulled = RuntimeConfig::from_snapshot(&snapshot, None);
        assert_eq!(pulled.morses.len(), 1, "the unused morse slots were kept");
        assert_eq!(pulled.combos.len(), 1, "the unused combo slots were kept");

        let text = pulled.to_toml().expect("serialize");
        let reparsed = RuntimeConfig::from_toml(&text).expect("a pulled file must parse again");
        assert_eq!(
            reparsed.snapshot().expect("snapshot").behaviors,
            config.snapshot().expect("snapshot").behaviors,
        );
    }

    /// An unused slot *inside* the table cannot be dropped: the keymap
    /// addresses a morse as `TD(n)` by position, so compacting the table would
    /// silently repoint every later key. It is written as a morse that holds
    /// nothing, which is what `KC_NO` means, and comes back as the same gap.
    #[test]
    fn an_unused_slot_between_two_used_ones_keeps_its_position() {
        let config = RuntimeConfig::from_toml(&two_behavior_tables()).expect("parse");
        let mut snapshot = config.snapshot().expect("snapshot");
        let morses = snapshot.behaviors.morses.as_mut().expect("morses");
        let used = morses[0].clone();
        *morses = vec![used.clone(), Morse::default(), used];

        let pulled = RuntimeConfig::from_snapshot(&snapshot, None);
        assert_eq!(pulled.morses.len(), 3, "the gap moved the last morse");

        let text = pulled.to_toml().expect("serialize");
        let reparsed = RuntimeConfig::from_toml(&text).expect("a pulled file must parse again");
        assert_eq!(
            reparsed.snapshot().expect("snapshot").behaviors.morses,
            snapshot.behaviors.morses,
        );
    }

    /// Firmware without the runtime conditional commands reports `None`, which
    /// must stay distinct from an empty table: a file naming no rules is
    /// satisfied either way, but a file naming rules is not.
    #[test]
    fn unsupported_conditional_table_only_conflicts_when_rules_are_named() {
        let unsupported = {
            let mut snap = lighting_snapshot(None, None);
            snap.conditional_scenes = None;
            Snapshot {
                behaviors: BehaviorSnapshot::default(),
                default_layer: 0,
                layers: Vec::new(),
                lighting: Some(snap),
            }
        };
        let empty_file = {
            let mut snap = lighting_snapshot(None, None);
            snap.conditional_scenes = Some(Vec::new());
            Snapshot {
                behaviors: BehaviorSnapshot::default(),
                default_layer: 0,
                layers: Vec::new(),
                lighting: Some(snap),
            }
        };
        assert!(differences(&empty_file, &unsupported).is_empty());

        let with_rule = {
            let mut snap = lighting_snapshot(None, None);
            snap.conditional_scenes = Some(vec![ConditionalSceneConfig {
                connection: None,
                led: 75,
                color: "#0040a0".into(),
                effect: EffectKind::Solid,
                period_ms: None,
                phase_ms: None,
                duty: None,
                step_ms: None,
                layer: None,
                battery: Some(BatteryConditionConfig {
                    node: 1,
                    min_level: Some(81),
                    max_level: None,
                    charge: ChargeConditionConfig::Charging,
                }),
                output_mode: None,
                effects: None,
            }]);
            Snapshot {
                behaviors: BehaviorSnapshot::default(),
                default_layer: 0,
                layers: Vec::new(),
                lighting: Some(snap),
            }
        };
        let found = differences(&with_rule, &unsupported);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("no runtime conditional table"));
    }

    #[test]
    fn conditional_rules_round_trip_through_the_wire_and_reject_bad_batteries() {
        let mut cell = ConditionalSceneConfig {
            connection: None,
            led: 75,
            color: "#0040a0".into(),
            effect: EffectKind::Solid,
            period_ms: None,
            phase_ms: None,
            duty: None,
            step_ms: None,
            layer: Some(LayerConditionConfig {
                layer: 3,
                active: false,
            }),
            battery: Some(BatteryConditionConfig {
                node: 1,
                min_level: Some(20),
                max_level: Some(80),
                charge: ChargeConditionConfig::Discharging,
            }),
            output_mode: None,
            effects: None,
        };
        let wire = conditional_scene_to_wire(&cell).unwrap();
        assert_eq!(conditional_scene_from_wire(wire), cell);
        assert!(validate_conditional_scene(0, &cell).is_ok());

        cell.connection = Some(ConnectionConditionConfig {
            transport: Some(TransportConfig::Ble),
            profile: Some(2),
            ble_state: Some(BleStateConfig::Connected),
            bonded: None,
            usb_connected: None,
        });
        let wire = conditional_scene_to_wire(&cell).unwrap();
        assert_eq!(conditional_scene_from_wire(wire), cell);
        assert!(validate_conditional_scene(0, &cell).is_ok());

        cell.connection = Some(ConnectionConditionConfig {
            transport: None,
            profile: Some(4),
            ble_state: None,
            bonded: None,
            usb_connected: None,
        });
        assert!(validate_conditional_scene(0, &cell).is_err());

        cell.connection = Some(ConnectionConditionConfig {
            transport: None,
            profile: None,
            ble_state: None,
            bonded: None,
            usb_connected: None,
        });
        assert!(validate_conditional_scene(0, &cell).is_err());
        cell.connection = None;

        // The firmware would decline these on apply; catching them offline
        // means `config validate` is enough to know a file is writable.
        cell.battery = Some(BatteryConditionConfig {
            node: 1,
            min_level: Some(90),
            max_level: Some(10),
            charge: ChargeConditionConfig::Any,
        });
        assert!(validate_conditional_scene(0, &cell).is_err());

        cell.battery = Some(BatteryConditionConfig {
            node: 1,
            min_level: Some(120),
            max_level: None,
            charge: ChargeConditionConfig::Any,
        });
        assert!(validate_conditional_scene(0, &cell).is_err());
    }

    fn lighting_snapshot(
        effects: Option<EffectsConfig>,
        params: Option<Vec<EffectParams>>,
    ) -> LightingSnapshot {
        LightingSnapshot {
            brightness: 100,
            output_mode: OutputModeConfig::AlwaysOn,
            scene_policy: ScenePolicyConfig::EffectiveOnly,
            background: BackgroundConfig {
                enabled: false,
                hue: 0,
                saturation: 0,
                value: 0,
                speed: 0,
                mode: BackgroundModeConfig::Solid,
            },
            effects,
            params,
            scenes: Vec::new(),
            conditional_scenes: Some(Vec::new()),
        }
    }

    fn effects_with(params: &[(&str, u8)]) -> EffectsConfig {
        EffectsConfig {
            effect: "Rain".into(),
            overlay: None,
            palette: "Aurora".into(),
            value: 200,
            speed: 40,
            params: BTreeMap::from([(
                "Rain".to_owned(),
                params
                    .iter()
                    .map(|(name, value)| ((*name).to_owned(), *value))
                    .collect(),
            )]),
        }
    }

    #[test]
    fn effect_param_tables_round_trip() {
        let config: LightingConfig = toml::from_str(LIGHTING_WITH_PARAMS).unwrap();
        assert_eq!(params_of(&config, "Rain", "Density"), Some(6));
        assert_eq!(params_of(&config, "Rain", "Trail Length"), Some(128));
        assert_eq!(
            config
                .effects
                .as_ref()
                .and_then(|effects| effects.overlay.as_deref()),
            Some("Reactive")
        );

        let text = toml::to_string_pretty(&config).unwrap();
        assert!(text.contains("[effects.params.Rain]"), "{text}");
        let reparsed: LightingConfig = toml::from_str(&text).unwrap();
        assert_eq!(reparsed.effects, config.effects);
    }

    #[test]
    fn effect_params_are_optional_and_omitted_when_empty() {
        let config: LightingConfig = toml::from_str(&LIGHTING_WITH_PARAMS.replace(
            "[effects.params.Rain]\nDensity = 6\n\"Trail Length\" = 128\n",
            "",
        ))
        .unwrap();
        assert!(config.effects.as_ref().unwrap().params.is_empty());
        assert!(!toml::to_string_pretty(&config).unwrap().contains("params"));
    }

    #[test]
    fn effect_param_names_must_not_be_empty() {
        let text = LIGHTING_WITH_PARAMS.replace("[effects.params.Rain]", "[effects.params.\"\"]");
        let error = toml::from_str::<LightingConfig>(&text)
            .unwrap()
            .snapshot()
            .unwrap_err();
        assert!(error.to_string().contains("empty effect name"), "{error}");

        let text = LIGHTING_WITH_PARAMS.replace("Density = 6", "\"\" = 6");
        let error = toml::from_str::<LightingConfig>(&text)
            .unwrap()
            .snapshot()
            .unwrap_err();
        assert!(
            error.to_string().contains("empty parameter name"),
            "{error}"
        );
    }

    #[test]
    fn effect_param_values_are_bytes() {
        let text = LIGHTING_WITH_PARAMS.replace("Density = 6", "Density = 256");
        assert!(toml::from_str::<LightingConfig>(&text).is_err());
    }

    #[test]
    fn pull_records_only_parameters_that_differ_from_their_default() {
        let snapshot = Snapshot {
            behaviors: BehaviorSnapshot::default(),
            default_layer: 0,
            layers: vec![vec![0; LAYER_SIZE]],
            lighting: Some(lighting_snapshot(
                Some(effects_with(&[("Density", 6), ("Trail Length", 128)])),
                Some(vec![param_set(
                    "Rain",
                    &[("Density", 0, 16, 4, 6), ("Trail Length", 0, 255, 128, 128)],
                )]),
            )),
        };
        let mut config = RuntimeConfig::from_snapshot(&snapshot, None);
        config.retain_non_default_params(&snapshot);
        let lighting = config.lighting.unwrap();
        assert_eq!(params_of(&lighting, "Rain", "Density"), Some(6));
        assert_eq!(params_of(&lighting, "Rain", "Trail Length"), None);
    }

    #[test]
    fn pull_drops_parameters_when_the_keyboard_has_none() {
        let snapshot = Snapshot {
            behaviors: BehaviorSnapshot::default(),
            default_layer: 0,
            layers: vec![vec![0; LAYER_SIZE]],
            lighting: Some(lighting_snapshot(
                Some(effects_with(&[("Density", 6)])),
                None,
            )),
        };
        let mut config = RuntimeConfig::from_snapshot(&snapshot, None);
        config.retain_non_default_params(&snapshot);
        assert!(config.lighting.unwrap().effects.unwrap().params.is_empty());
    }

    #[test]
    fn parameter_differences_only_cover_what_the_file_names() {
        let desired = lighting_snapshot(Some(effects_with(&[("Density", 6)])), None);
        let live = lighting_snapshot(
            Some(effects_with(&[("Density", 4), ("Trail Length", 200)])),
            Some(vec![param_set(
                "Rain",
                &[("Density", 0, 16, 4, 4), ("Trail Length", 0, 255, 128, 200)],
            )]),
        );
        assert_eq!(
            param_differences(&desired, &live),
            vec!["lighting parameter Rain.Density: file 6 != keyboard 4"]
        );

        let matching = lighting_snapshot(Some(effects_with(&[("Density", 4)])), None);
        assert!(param_differences(&matching, &live).is_empty());
    }

    #[test]
    fn unadvertised_parameters_are_reported_as_differences() {
        let desired = lighting_snapshot(Some(effects_with(&[("Sparkle", 3)])), None);
        let live = lighting_snapshot(Some(effects_with(&[])), None);
        assert_eq!(
            param_differences(&desired, &live),
            vec!["lighting parameter Rain.Sparkle: file 3 != keyboard (not advertised)"]
        );
    }

    #[test]
    fn parameter_tables_do_not_disturb_the_extension_selection_diff() {
        let desired = lighting_snapshot(Some(effects_with(&[("Density", 6)])), None);
        let live = lighting_snapshot(Some(effects_with(&[])), None);
        assert_eq!(
            desired.effects.as_ref().map(EffectsConfig::selection),
            live.effects.as_ref().map(EffectsConfig::selection)
        );
    }
}
