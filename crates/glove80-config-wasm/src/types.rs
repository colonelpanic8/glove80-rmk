//! The values crossing the wasm ABI.
//!
//! Every field is either a `rynk` protocol type or a plain scalar, so a host
//! that already holds device state can pass it straight in without first
//! rewriting it into the file format's names and colour strings. tsify gives
//! each of these a named TypeScript declaration.

use rynk::rmk_types::action::KeyAction;
use rynk::rmk_types::auto_mouse::AutoMouseLayerConfig;
use rynk::rmk_types::combo::Combo;
use rynk::rmk_types::fork::Fork;
use rynk::rmk_types::morse::{Morse, MorseProfile};
use rynk::rmk_types::protocol::rynk::{
    BehaviorConfig as WireBehaviorConfig, BehaviorOptions, LightingBackgroundState,
    LightingExtendedConditionalSceneCell, LightingExtensionParam, LightingExtensionState,
    LightingLayerPolicy, LightingOutputMode, LightingSceneCell,
};
use serde::{Deserialize, Serialize};
use tsify::Tsify;

/// Which source format a document is written in.
///
/// The CLI infers this from a path's extension. A browser has no path — a file
/// arrives as text from a picker or a drop — so the format is recognized from
/// the document itself and handed back to the caller, which is what lets an
/// export write the same kind of file the user imported.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigFormat {
    #[default]
    Toml,
    /// The experimental JSON backup format from the MoErgo Layout Editor.
    MoergoJson,
}

/// A parsed document together with the format it turned out to be, and whatever
/// the parse had to say about it.
#[derive(Clone, Debug, Deserialize, Serialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi)]
pub struct ParsedConfig {
    pub format: ConfigFormat,
    pub snapshot: RuntimeSnapshot,
    /// Bindings that did not survive the trip exactly. Empty for a TOML
    /// document, which describes the managed state directly and so has nothing
    /// to approximate; an editor export is where a layout meets a keyboard that
    /// expresses things differently.
    ///
    /// A parse that returns these still succeeded. They are the difference
    /// between what the export asked for and what the keyboard will do, and
    /// dropping them on the floor is how an import comes to look lossless when
    /// it was not.
    pub notes: Vec<ImportNote>,
}

/// One way an imported layout differs from its source.
#[derive(Clone, Debug, Deserialize, Serialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi)]
pub struct ImportNote {
    /// True when the binding was imported but behaves differently; false when
    /// it could not be represented at all. A parse only succeeds if every note
    /// is an approximation, so the strict form never reaches a caller — it is
    /// carried anyway so a report can say which kind it is showing.
    pub approximated: bool,
    /// Where in the source document, e.g. `layer 3 (Cursor), editor key 41`.
    pub location: Option<String>,
    pub message: String,
}

/// The managed runtime state as a browser holds it: the same protocol types the
/// device reports, rather than the TOML file's names and colour strings.
#[derive(Clone, Debug, Deserialize, Serialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi)]
pub struct RuntimeSnapshot {
    /// Persistent BLE advertising-name template, when supported.
    #[serde(default)]
    pub bluetooth_name: Option<String>,
    pub default_layer: u8,
    /// One entry per layer, each row-major over the 6x14 grid.
    pub layers: Vec<Vec<KeyAction>>,
    pub lighting: Option<LightingSnapshot>,
    #[serde(default)]
    pub behaviors: BehaviorSnapshot,
}

/// The tables a keymap cell addresses by index: morses for `TD(n)`, macros for
/// `TriggerMacro(n)`, and the combos that fire alongside them.
///
/// Each field distinguishes silence from emptiness the way the lighting fields
/// do: `undefined` means the source says nothing about that table and it must be
/// left as the device holds it, so a document written before these existed can
/// never read as "clear them".
#[derive(Clone, Debug, Default, Deserialize, Serialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi)]
pub struct BehaviorSnapshot {
    pub config: Option<WireBehaviorConfig>,
    pub options: Option<BehaviorOptions>,
    pub morse_profiles: Option<Vec<MorseProfile>>,
    pub auto_mouse_layers: Option<Vec<AutoMouseLayerConfig>>,
    pub morses: Option<Vec<Morse>>,
    pub combos: Option<Vec<Combo>>,
    /// Macro space exactly as the firmware stores it: the sequences
    /// concatenated, each closed by its own terminator, which is what
    /// `TriggerMacro` indexes into.
    pub macros: Option<Vec<u8>>,
    /// Forks: one key's output swapped while a modifier is held. Not addressed
    /// by index from a keymap cell — a fork matches the action it replaces.
    pub forks: Option<Vec<Fork>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi)]
pub struct LightingSnapshot {
    pub brightness: u8,
    pub output_mode: LightingOutputMode,
    pub scene_policy: LightingLayerPolicy,
    pub background: LightingBackgroundState,
    /// None when the file names no extension effect, or the device exposes none.
    pub effects: Option<LightingExtensionState>,
    /// The optional second effect layered over `effects`, indexing the same
    /// advertised effect list. It travels separately on the wire
    /// (`LightingExtensionLayers`), so it does so here too.
    pub overlay: Option<u8>,
    /// Parameter values this snapshot asserts, already resolved to the ordinals
    /// the protocol addresses.
    pub effect_params: Vec<EffectParamWrite>,
    pub scenes: Vec<LightingSceneCell>,
    /// The mutable, ordered conditional table. `undefined` means the firmware
    /// has no such table at all, which stays distinct from a supported-but-empty
    /// one: a file naming rules conflicts with the former and not the latter.
    /// Cells are the extended form; hosts talking to older firmware pass
    /// `connection: undefined` on every cell.
    pub conditional_scenes: Option<Vec<LightingExtendedConditionalSceneCell>>,
}

/// One parameter value addressed the way `SetLightingExtensionParam` addresses
/// it: by the effect's index in the advertised effect list, and the parameter's
/// ordinal within that effect's own list.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi)]
pub struct EffectParamWrite {
    pub effect: u8,
    pub index: u8,
    pub value: u8,
}

/// What the connected keyboard advertises. The file speaks in effect, palette
/// and parameter *names*; the protocol speaks in indices and ordinals, so a
/// round-trip needs the device's own lists to translate between them.
#[derive(Clone, Debug, Deserialize, Serialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi)]
pub struct ExtensionCatalog {
    pub effects: Vec<String>,
    pub palettes: Vec<String>,
    /// Advertised parameters per effect, in firmware order. Effects with no
    /// parameters are simply absent, matching how the CLI reads them.
    pub params: Vec<EffectParamSet>,
}

/// One effect's advertised parameters. `LightingExtensionParam` carries each
/// parameter's bounds, its firmware default, and its live value in one row, so
/// this is enough to validate a file and to prune defaults out of a pulled one.
#[derive(Clone, Debug, Deserialize, Serialize, Tsify)]
#[tsify(into_wasm_abi, from_wasm_abi)]
pub struct EffectParamSet {
    /// Index of the effect within `ExtensionCatalog::effects`.
    pub effect: u8,
    pub name: String,
    pub params: Vec<LightingExtensionParam>,
}
