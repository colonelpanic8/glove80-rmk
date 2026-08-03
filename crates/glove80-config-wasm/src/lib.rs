//! The `glove80.toml` round-trip, compiled to a wasm package.
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

use glove80_config::{differences, RuntimeConfig};
use wasm_bindgen::prelude::*;

pub use types::{
    EffectParamSet, EffectParamWrite, ExtensionCatalog, LightingSnapshot, RuntimeSnapshot,
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

/// Parse and validate a `glove80.toml`, then restate it in protocol types.
///
/// The catalog resolves the file's effect, palette and parameter names to the
/// indices and ordinals the protocol addresses, and its advertised bounds are
/// checked here rather than left to the firmware, so an out-of-range parameter
/// is reported by name instead of as a bare rejection.
#[wasm_bindgen]
pub fn parse_config(text: &str, catalog: ExtensionCatalog) -> Result<RuntimeSnapshot, JsValue> {
    let config = RuntimeConfig::from_toml(text).map_err(js_error)?;
    let snapshot = config.snapshot().map_err(js_error)?;
    convert::snapshot_to_wire(&snapshot, &catalog).map_err(js_error)
}

/// Render live device state as a `glove80.toml`, the way `config pull` does.
///
/// `previous` is the file being replaced. The firmware stores no layer labels,
/// so without it every `[[layer]]` comes back as a synthesized `layerN` /
/// `Layer N`; with it the user's own `id` and `name` survive the round-trip. A
/// `previous` that no longer parses is ignored rather than fatal, matching the
/// CLI's `parse(file).ok()`.
#[wasm_bindgen]
pub fn render_config(
    snapshot: RuntimeSnapshot,
    catalog: ExtensionCatalog,
    previous: Option<String>,
) -> Result<String, JsValue> {
    let mut snapshot = convert::snapshot_from_wire(&snapshot, &catalog).map_err(js_error)?;
    glove80_config::trim_trailing_transparent_layers(&mut snapshot.layers);
    let labels = previous
        .as_deref()
        .and_then(|text| RuntimeConfig::from_toml(text).ok());
    let mut config = RuntimeConfig::from_snapshot(&snapshot, labels.as_ref());
    // A rendered file records only what the user actually tuned; parameters
    // still holding their firmware default are noise, and a file that omits a
    // parameter leaves it alone rather than resetting it.
    config.retain_non_default_params(&snapshot);
    config.to_toml().map_err(js_error)
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

/// Parse and validate only. Deliberately offline, like `config validate`: no
/// catalog, so a file can be checked before any keyboard is connected. Anything
/// that depends on what a particular device advertises — effect and palette
/// names, parameter bounds — is therefore *not* checked here.
#[wasm_bindgen]
pub fn validate_config(text: &str) -> Result<(), JsValue> {
    RuntimeConfig::from_toml(text).map(|_| ()).map_err(js_error)
}
