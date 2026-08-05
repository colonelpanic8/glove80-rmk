//! Translation between the wire-shaped [`crate::types`] and `glove80-config`'s
//! internal [`model::Snapshot`].
//!
//! Nothing here decides anything about the file format; every rule — validation,
//! colour canonicalization, name-to-index resolution, parameter bounds — comes
//! out of `glove80-config`, which is the same code the CLI runs. This module
//! only rearranges values.

use glove80_config as model;
use rynk::rmk_types::protocol::rynk::LightingExtensionState;

use crate::types::{
    BehaviorSnapshot, EffectParamWrite, ExtensionCatalog, LightingSnapshot, RuntimeSnapshot,
};

/// The catalog restated as the model's advertised-parameter type. The model
/// keys effects by name because that is what a file writes; the browser holds
/// them by index because that is what the protocol addresses.
fn advertised(catalog: &ExtensionCatalog) -> Vec<model::EffectParams> {
    catalog
        .params
        .iter()
        .map(|set| model::EffectParams {
            index: set.effect,
            effect: set.name.clone(),
            params: set
                .params
                .iter()
                .map(|param| model::ParamSpec {
                    name: param.name.as_str().to_owned(),
                    min: param.min,
                    max: param.max,
                    default: param.default,
                    value: param.value,
                })
                .collect(),
        })
        .collect()
}

/// Device state as the model sees it.
///
/// The layer table is deliberately left at the device's full fixed capacity:
/// `differences` only walks as far as the file's own layer count, so trimming
/// here would change nothing for a diff while silently dropping layers a caller
/// meant to keep. Only [`crate::render_config`] trims, because only a file has
/// to be short.
pub fn snapshot_from_wire(
    snapshot: &RuntimeSnapshot,
    catalog: &ExtensionCatalog,
) -> anyhow::Result<model::Snapshot> {
    let layers = snapshot
        .layers
        .iter()
        .enumerate()
        .map(|(layer, actions)| {
            if actions.len() != model::LAYER_SIZE {
                anyhow::bail!(
                    "layer {layer} has {} keys, expected the Glove80's {}",
                    actions.len(),
                    model::LAYER_SIZE
                );
            }
            actions
                .iter()
                .copied()
                .enumerate()
                .map(|(offset, action)| model::action_to_code(action, layer, offset))
                .collect::<anyhow::Result<Vec<_>>>()
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    let lighting = snapshot
        .lighting
        .as_ref()
        .map(|lighting| lighting_from_wire(lighting, catalog))
        .transpose()?;

    Ok(model::Snapshot {
        default_layer: snapshot.default_layer,
        layers,
        lighting,
        behaviors: model::BehaviorSnapshot {
            morses: snapshot.behaviors.morses.clone(),
            combos: snapshot.behaviors.combos.clone(),
            macros: snapshot.behaviors.macros.clone(),
        },
    })
}

fn lighting_from_wire(
    lighting: &LightingSnapshot,
    catalog: &ExtensionCatalog,
) -> anyhow::Result<model::LightingSnapshot> {
    let sets = advertised(catalog);
    let effects = lighting
        .effects
        .map(|state| {
            let writes = lighting
                .effect_params
                .iter()
                .map(|write| (write.effect, write.index, write.value))
                .collect::<Vec<_>>();
            model::effects_from_wire(
                state,
                lighting.overlay,
                &catalog.effects,
                &catalog.palettes,
                model::param_tables_from_wire(&writes, &sets)?,
            )
        })
        .transpose()?;

    // The model treats a scene table as an unordered set addressed by
    // (layer, LED), and both the file and the device paths sort it before
    // comparing, so ordering never reads as a difference.
    let mut scenes = lighting
        .scenes
        .iter()
        .copied()
        .map(model::scene_from_wire)
        .collect::<Vec<_>>();
    scenes.sort();

    Ok(model::LightingSnapshot {
        brightness: lighting.brightness,
        output_mode: model::output_mode_from_wire(lighting.output_mode),
        scene_policy: model::scene_policy_from_wire(lighting.scene_policy),
        background: model::background_from_wire(lighting.background),
        effects,
        // The catalog is the keyboard's own parameter report, so it is what a
        // parameter difference is measured against and what `pull` prunes
        // firmware defaults with.
        params: Some(sets),
        scenes,
        conditional_scenes: lighting.conditional_scenes.as_ref().map(|cells| {
            cells
                .iter()
                .copied()
                .map(model::conditional_scene_from_wire)
                .collect()
        }),
    })
}

/// The inverse: a validated model snapshot restated in protocol types, ready to
/// be written to a device.
pub fn snapshot_to_wire(
    snapshot: &model::Snapshot,
    catalog: &ExtensionCatalog,
) -> anyhow::Result<RuntimeSnapshot> {
    let layers = snapshot
        .layers
        .iter()
        .map(|keys| {
            keys.iter()
                .copied()
                .map(model::rynk_keycode::from_via_keycode)
                .collect()
        })
        .collect();

    let lighting = snapshot
        .lighting
        .as_ref()
        .map(|lighting| lighting_to_wire(lighting, catalog))
        .transpose()?;

    Ok(RuntimeSnapshot {
        default_layer: snapshot.default_layer,
        layers,
        lighting,
        behaviors: BehaviorSnapshot {
            morses: snapshot.behaviors.morses.clone(),
            combos: snapshot.behaviors.combos.clone(),
            macros: snapshot.behaviors.macros.clone(),
        },
    })
}

fn lighting_to_wire(
    lighting: &model::LightingSnapshot,
    catalog: &ExtensionCatalog,
) -> anyhow::Result<LightingSnapshot> {
    let sets = advertised(catalog);
    let mut state: Option<LightingExtensionState> = None;
    let mut overlay = None;
    let mut effect_params = Vec::new();
    if let Some(effects) = &lighting.effects {
        let (resolved, resolved_overlay) =
            model::effects_to_wire(effects, &catalog.effects, &catalog.palettes)?;
        state = Some(resolved);
        overlay = resolved_overlay;
        // `params_to_writes` is the same resolution and the same bounds check
        // the CLI's `apply_params` performs, so a file the browser accepts is
        // one the CLI would also have written.
        effect_params = model::params_to_writes(&effects.params, Some(&sets))?
            .into_iter()
            .map(|write| EffectParamWrite {
                effect: write.effect,
                index: write.index,
                value: write.value,
            })
            .collect();
    }

    Ok(LightingSnapshot {
        brightness: lighting.brightness,
        output_mode: model::output_mode_to_wire(lighting.output_mode),
        scene_policy: model::scene_policy_to_wire(lighting.scene_policy),
        background: model::background_to_wire(&lighting.background),
        effects: state,
        overlay,
        effect_params,
        scenes: lighting
            .scenes
            .iter()
            .map(model::scene_to_wire)
            .collect::<anyhow::Result<Vec<_>>>()?,
        conditional_scenes: lighting
            .conditional_scenes
            .as_ref()
            .map(|cells| {
                cells
                    .iter()
                    .map(model::conditional_scene_to_wire)
                    .collect::<anyhow::Result<Vec<_>>>()
            })
            .transpose()?,
    })
}
