//! The configuration round-trip, compiled to a wasm package.
//!
//! Two source formats are supported, the same two the CLI reads and writes: the
//! native `glove80.toml`, which describes the whole managed runtime state, and
//! the MoErgo Layout Editor's JSON backup, which describes only a keymap.
//!
//! A browser configurator holds live device state as `rynk` protocol types; a
//! source file speaks in effect names, keycode mnemonics and colour strings.
//! This package is the bridge, and it is deliberately thin: every rule about
//! the file format lives in `glove80-config`, which is exactly the code the
//! `glove80-control` CLI runs, so the two hosts cannot disagree about what a
//! file means.
//!
//! The format is Glove80-specific, not generic Rynk. `glove80-config` hardcodes
//! the 6x14 matrix and the four physical holes at r0c5, r0c8, r5c5 and r5c8; a
//! different Rynk board needs a different schema, not a different catalog.
//!
//! Native targets compile an empty crate. Build with
//! `wasm-pack build --release --target web`.
#![cfg(target_arch = "wasm32")]

mod convert;
mod types;

use glove80_config::{
    differences, runtime_config_from_moergo_json, snapshot_to_moergo_json, RuntimeConfig,
};
use wasm_bindgen::prelude::*;

pub use types::{
    ConfigFormat, EffectParamSet, EffectParamWrite, ExtensionCatalog, LightingSnapshot,
    ParsedConfig, RuntimeSnapshot,
};

#[wasm_bindgen(start)]
pub fn init() {
    console_error_panic_hook::set_once();
}

/// Flatten an `anyhow` chain into the one string the CLI would have printed.
/// The validation messages are the whole point of running this in a browser, so
/// they must read identically wherever they surface.
fn js_error(error: anyhow::Error) -> JsValue {
    JsError::new(&format!("{error:#}")).into()
}

/// Recognize a document's format from its text.
///
/// This is the CLI's `file_format` rule minus the filename half: a document
/// that arrives through a file picker, a drop, or the clipboard has no
/// extension to consult, so only the leading `{` of a JSON object distinguishes
/// the two. A TOML file cannot start that way, which is what makes the test
/// safe rather than merely convenient.
#[wasm_bindgen]
pub fn detect_config_format(text: &str) -> ConfigFormat {
    if text.trim_start().starts_with('{') {
        ConfigFormat::MoergoJson
    } else {
        ConfigFormat::Toml
    }
}

/// Parse a document of either supported format into the managed runtime model.
fn parse_text(text: &str, format: ConfigFormat) -> anyhow::Result<RuntimeConfig> {
    match format {
        ConfigFormat::Toml => RuntimeConfig::from_toml(text),
        ConfigFormat::MoergoJson => runtime_config_from_moergo_json(text),
    }
}

/// Parse and validate a configuration document, then restate it in protocol
/// types. The format is recognized from the text, so a caller can hand over
/// whatever the user picked without inspecting it first.
///
/// The catalog resolves the file's effect, palette and parameter names to the
/// indices and ordinals the protocol addresses, and its advertised bounds are
/// checked here rather than left to the firmware, so an out-of-range parameter
/// is reported by name instead of as a bare rejection. A MoErgo document
/// carries no lighting at all, so for that format the catalog goes unused.
#[wasm_bindgen]
pub fn parse_config(text: &str, catalog: ExtensionCatalog) -> Result<RuntimeSnapshot, JsValue> {
    let config = parse_text(text, detect_config_format(text)).map_err(js_error)?;
    let snapshot = config.snapshot().map_err(js_error)?;
    convert::snapshot_to_wire(&snapshot, &catalog).map_err(js_error)
}

/// [`parse_config`], reporting which format the document turned out to be.
///
/// An importer needs that answer back: the natural thing to write on a later
/// export is the kind of file the user brought in, and only the parse knows
/// which that was.
#[wasm_bindgen]
pub fn parse_config_document(
    text: &str,
    catalog: ExtensionCatalog,
) -> Result<ParsedConfig, JsValue> {
    let format = detect_config_format(text);
    let snapshot = parse_config(text, catalog)?;
    Ok(ParsedConfig { format, snapshot })
}

/// Render live device state as a source document, the way `config pull` does.
///
/// `previous` is the file being replaced, and it does double duty exactly as it
/// does in the CLI. Whatever its format, it supplies the layer labels: the
/// firmware stores none, so without it every layer comes back as a synthesized
/// `layerN` / `Layer N`. When the output is MoErgo JSON it is *also* the
/// template, which is what preserves the editor-owned fields Rynk does not
/// manage — macros, combos, custom behaviors, the document's own identity. A
/// `previous` that no longer parses is ignored rather than fatal, matching the
/// CLI's `parse_text(...).ok()`.
#[wasm_bindgen]
pub fn render_config_as(
    snapshot: RuntimeSnapshot,
    catalog: ExtensionCatalog,
    format: ConfigFormat,
    previous: Option<String>,
) -> Result<String, JsValue> {
    let mut snapshot = convert::snapshot_from_wire(&snapshot, &catalog).map_err(js_error)?;
    glove80_config::trim_trailing_transparent_layers(&mut snapshot.layers);
    let labels = previous
        .as_deref()
        .and_then(|text| parse_text(text, detect_config_format(text)).ok());
    let mut config = RuntimeConfig::from_snapshot(&snapshot, labels.as_ref());
    // A rendered file records only what the user actually tuned; parameters
    // still holding their firmware default are noise, and a file that omits a
    // parameter leaves it alone rather than resetting it.
    config.retain_non_default_params(&snapshot);
    match format {
        ConfigFormat::Toml => config.to_toml(),
        // Only a document that already *is* MoErgo JSON can serve as a
        // template; handing `snapshot_to_moergo_json` a TOML file would fail
        // the parse it does up front.
        ConfigFormat::MoergoJson => snapshot_to_moergo_json(
            &snapshot,
            Some(&config),
            previous
                .as_deref()
                .filter(|text| detect_config_format(text) == ConfigFormat::MoergoJson),
        ),
    }
    .map_err(js_error)
}

/// [`render_config_as`] fixed to TOML, which is the format a `glove80.toml`
/// round-trip means when no one has said otherwise.
#[wasm_bindgen]
pub fn render_config(
    snapshot: RuntimeSnapshot,
    catalog: ExtensionCatalog,
    previous: Option<String>,
) -> Result<String, JsValue> {
    render_config_as(snapshot, catalog, ConfigFormat::Toml, previous)
}

/// [`render_config_as`] fixed to the MoErgo Layout Editor's JSON backup.
///
/// `template` is the document being replaced. Passing the file the user
/// imported is what makes an import/export round-trip lossless for everything
/// outside the keymap; passing nothing produces a minimal document from
/// scratch, which the editor accepts but which carries none of that history.
#[wasm_bindgen]
pub fn render_moergo_json(
    snapshot: RuntimeSnapshot,
    catalog: ExtensionCatalog,
    template: Option<String>,
) -> Result<String, JsValue> {
    render_config_as(snapshot, catalog, ConfigFormat::MoergoJson, template)
}

/// The difference lines `config diff` prints, in the same order and wording.
///
/// Both sides go through the catalog so effect and palette *names* appear in
/// the report, which is what makes a selection difference readable.
#[wasm_bindgen]
pub fn diff_config(
    desired: RuntimeSnapshot,
    live: RuntimeSnapshot,
    catalog: ExtensionCatalog,
) -> Result<Vec<String>, JsValue> {
    let desired = convert::snapshot_from_wire(&desired, &catalog).map_err(js_error)?;
    let live = convert::snapshot_from_wire(&live, &catalog).map_err(js_error)?;
    Ok(differences(&desired, &live))
}

/// Parse and validate only, reporting the format that was recognized.
/// Deliberately offline, like `config validate`: no catalog, so a file can be
/// checked before any keyboard is connected. Anything that depends on what a
/// particular device advertises — effect and palette names, parameter bounds —
/// is therefore *not* checked here.
#[wasm_bindgen]
pub fn validate_config(text: &str) -> Result<ConfigFormat, JsValue> {
    let format = detect_config_format(text);
    parse_text(text, format).map_err(js_error)?;
    Ok(format)
}
