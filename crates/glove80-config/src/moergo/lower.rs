//! Lowering of a MoErgo export's ZMK behaviors onto the Rynk runtime model.
//!
//! ZMK expresses a custom behavior as a devicetree node that a key binding
//! references by name. Rynk has no such indirection: a keymap cell holds a
//! typed action, and anything with its own timing is a [`Morse`] in a table the
//! cell addresses by index. Lowering therefore allocates one morse per distinct
//! (behavior, parameters) triple actually used by a layer and rewrites the cell
//! to `TD(n)`.
//!
//! Everything produced here is runtime state. The morse table, the combo table
//! and the keymap all have Rynk setters, so an import lands over the wire; only
//! the table *capacities* and the hand tags that [`unilateral_tap`] reads are
//! compile-time, and those are properties of the firmware rather than of any
//! one layout.
//!
//! [`unilateral_tap`]: rynk::rmk_types::morse::MorseProfile::unilateral_tap

use anyhow::{bail, Context, Result};
use rynk::rmk_types::action::{Action, KeyAction};
use rynk::rmk_types::combo::Combo;
use rynk::rmk_types::fork::{Fork, StateBits};
use rynk::rmk_types::modifier::ModifierCombination;
use rynk::rmk_types::morse::{Morse, MorseMode, MorseProfile, HOLD, TAP};
use serde_json::Value;

use super::behaviors::{BehaviorTables, Binding, HoldTap};
use crate::keycodes;
use crate::rynk_keycode::from_via_keycode;

/// Something the import could not represent, named by where it came from.
///
/// Import does not fail on these: a layout is usually mostly portable, and a
/// report of the exact keys that are not is more useful than a single error
/// that hides the rest of the work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// The binding has no equivalent and the key was left empty.
    Dropped,
    /// The binding was imported, but the result is not exactly what the export
    /// asked for.
    Approximated,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    /// Human-readable source location, e.g. `layer 3 (Cursor), editor key 41`.
    pub location: Option<String>,
    pub message: String,
}

impl Diagnostic {
    pub(crate) fn new(
        severity: Severity,
        location: Option<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            severity,
            location,
            message: message.into(),
        }
    }
}

/// The bilateral home-row-mod idiom, recovered from a set of hold-taps.
///
/// ZMK cannot express "this hold is reachable only from the other hand", so
/// TailorKey-style layouts build it out of one momentary layer per finger: the
/// hold binding presses the modifier *and* activates that finger's layer, whose
/// same-hand keys are rebound to variants that force the held key back to a
/// tap. Rynk computes the same decision from the layout's hand tags, so the
/// finger layers and their chained macros carry no information here and are
/// dropped.
#[derive(Debug, Default)]
pub(crate) struct HrmIdiom {
    /// Editor layer indices that exist only to serve the idiom.
    pub finger_layers: Vec<usize>,
}

impl HrmIdiom {
    /// A hold-tap belongs to the idiom when it restricts its hold to a set of
    /// key positions, which is the only thing ZMK offers for the purpose.
    pub fn is_member(hold_tap: &HoldTap) -> bool {
        !hold_tap.hold_trigger_key_positions.is_empty()
    }

    pub fn detect(tables: &BehaviorTables) -> Self {
        let mut finger_layers = Vec::new();
        for hold_tap in tables.hold_taps.iter().filter(|ht| Self::is_member(ht)) {
            // The hold side is a macro that presses the modifier and turns on
            // the finger layer; the `&mo` inside it names the layer to drop.
            let Some(hold_macro) = tables.macro_named(hold_tap.hold_binding()) else {
                continue;
            };
            for binding in &hold_macro.bindings {
                if binding.name() != "&mo" {
                    continue;
                }
                if let Some(layer) = binding.param_u8(0) {
                    let layer = layer as usize;
                    if !finger_layers.contains(&layer) {
                        finger_layers.push(layer);
                    }
                }
            }
        }
        finger_layers.sort_unstable();
        Self { finger_layers }
    }
}

/// Allocates the morse table while rewriting keymap cells, and collects
/// everything that did not survive the trip.
pub(crate) struct Lowering<'a> {
    tables: &'a BehaviorTables,
    hrm: HrmIdiom,
    /// Editor layer index -> Rynk layer index, with dropped layers absent.
    layer_map: Vec<Option<u8>>,
    morses: Vec<Morse>,
    /// Encoded macro sequences, in the byte format Rynk's macro space uses.
    macros: Vec<Vec<u8>>,
    /// Forks recovered from the export's mod-morphs.
    forks: Vec<Fork>,
    diagnostics: Vec<Diagnostic>,
}

/// A ZMK modifier-name list (`MOD_LSFT`) as an RMK modifier combination.
fn zmk_modifier_combination(mods: &[String]) -> Result<ModifierCombination> {
    let mut combination = ModifierCombination::default();
    for name in mods {
        combination = match name.trim().to_ascii_uppercase().as_str() {
            "MOD_LCTL" => combination.with_left_ctrl(true),
            "MOD_LSFT" => combination.with_left_shift(true),
            "MOD_LALT" => combination.with_left_alt(true),
            "MOD_LGUI" => combination.with_left_gui(true),
            "MOD_RCTL" => combination.with_right_ctrl(true),
            "MOD_RSFT" => combination.with_right_shift(true),
            "MOD_RALT" => combination.with_right_alt(true),
            "MOD_RGUI" => combination.with_right_gui(true),
            other => bail!("'{other}' is not a ZMK modifier"),
        };
    }
    Ok(combination)
}

/// The same list as the fork condition's state bits.
fn zmk_state_bits(mods: &[String]) -> Result<StateBits> {
    Ok(StateBits {
        modifiers: zmk_modifier_combination(mods)?,
        ..StateBits::default()
    })
}

/// The HID keycodes for a VIA packed-modifier byte, in a stable order so two
/// identical macros encode identically and share a slot.
fn modifier_keys(packed: u8) -> Vec<u8> {
    const MODIFIERS: [(u8, u8); 8] = [
        (0b0000_0001, 0xe0), // LCtrl
        (0b0000_0010, 0xe1), // LShift
        (0b0000_0100, 0xe2), // LAlt
        (0b0000_1000, 0xe3), // LGui
        (0b0001_0001, 0xe4), // RCtrl
        (0b0001_0010, 0xe5), // RShift
        (0b0001_0100, 0xe6), // RAlt
        (0b0001_1000, 0xe7), // RGui
    ];
    let right = packed & 0b0001_0000 != 0;
    MODIFIERS
        .iter()
        .filter(|(bit, _)| {
            let is_right = bit & 0b0001_0000 != 0;
            is_right == right && packed & bit & 0b0000_1111 != 0
        })
        .map(|(_, key)| *key)
        .collect()
}

impl<'a> Lowering<'a> {
    pub fn new(tables: &'a BehaviorTables, layer_count: usize) -> Self {
        let hrm = HrmIdiom::detect(tables);
        let mut layer_map = Vec::with_capacity(layer_count);
        let mut next = 0u8;
        for editor_layer in 0..layer_count {
            if hrm.finger_layers.contains(&editor_layer) {
                layer_map.push(None);
            } else {
                layer_map.push(Some(next));
                next += 1;
            }
        }
        Self {
            tables,
            hrm,
            layer_map,
            morses: Vec::new(),
            macros: Vec::new(),
            forks: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    pub fn finger_layers(&self) -> &[usize] {
        &self.hrm.finger_layers
    }

    /// The Rynk index of an editor layer, or `None` when that layer was
    /// dropped as part of the home-row-mod idiom.
    pub fn remap_layer(&self, editor_layer: usize) -> Option<u8> {
        self.layer_map.get(editor_layer).copied().flatten()
    }

    /// The whole remap, so a caller can hold it across a loop that also needs
    /// `&mut self` to allocate morses.
    pub fn layer_map(&self) -> &[Option<u8>] {
        &self.layer_map
    }

    pub fn morses(&self) -> &[Morse] {
        &self.morses
    }

    pub fn forks(&self) -> &[Fork] {
        &self.forks
    }

    pub fn macros(&self) -> &[Vec<u8>] {
        &self.macros
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn report(
        &mut self,
        severity: Severity,
        location: Option<String>,
        message: impl Into<String>,
    ) {
        self.diagnostics
            .push(Diagnostic::new(severity, location, message));
    }

    /// Records a key the import cannot represent and leaves the cell empty.
    ///
    /// A layout is usually mostly portable, so refusing the whole import over
    /// one binding hides the rest of the work. The cell becomes `KC_NO` and the
    /// diagnostic names it, which is what makes the gap reviewable instead of
    /// silent.
    fn unmapped(&mut self, here: &dyn Fn() -> String, message: impl Into<String>) -> u16 {
        self.diagnostics
            .push(Diagnostic::new(Severity::Dropped, Some(here()), message));
        0
    }

    /// [`Self::unmapped`] for a whole binding that failed to convert.
    ///
    /// Every error raised while reading a binding already names its own
    /// location, so the prefix is stripped rather than repeated: the diagnostic
    /// carries the location in its own field, which is what lets a report group
    /// by it.
    pub(crate) fn unmapped_binding(&mut self, location: String, error: &anyhow::Error) -> u16 {
        let full = format!("{error:#}");
        let message = full
            .strip_prefix(&location)
            .map(|rest| rest.trim_start_matches([' ', ':']).to_owned())
            .unwrap_or(full);
        self.diagnostics
            .push(Diagnostic::new(Severity::Dropped, Some(location), message));
        0
    }

    /// Interns a morse, so repeated bindings of the same behavior with the same
    /// parameters (every `&AS_v1_TKZ` on a row, say) share one table slot.
    fn intern(&mut self, morse: Morse) -> Result<u16> {
        let index = match self.morses.iter().position(|entry| *entry == morse) {
            Some(index) => index,
            None => {
                self.morses.push(morse);
                self.morses.len() - 1
            }
        };
        let index = u8::try_from(index).map_err(|_| {
            anyhow::anyhow!("layout needs more than 256 morse keys, which Rynk cannot address")
        })?;
        Ok(0x5700 | index as u16)
    }

    /// Resolves a binding that names a custom behavior, returning the VIA
    /// keycode the keymap cell should hold.
    ///
    /// `params` are the editor's positional parameters for the cell and `here`
    /// names the cell for diagnostics.
    pub fn resolve(
        &mut self,
        name: &str,
        params: &[Value],
        here: &dyn Fn() -> String,
    ) -> Result<u16> {
        if let Some(hold_tap) = self.tables.hold_tap(name) {
            return self.lower_hold_tap(&hold_tap.clone(), params, here);
        }
        if let Some(entry) = self.tables.macro_named(name) {
            let entry = entry.clone();
            if let Some((hold_tap, forwarded)) = self.forwarded_hold_tap(&entry, params) {
                return self.lower_hold_tap(&hold_tap, &forwarded, here);
            }
            if let Some(code) = self.lower_macro(&entry) {
                return Ok(code);
            }
            return Ok(self.unmapped(
                here,
                format!("macro behavior '{name}' has no Rynk equivalent"),
            ));
        }
        if let Some(morph) = self.tables.mod_morph_named(name) {
            return self.lower_mod_morph(&morph.clone(), here);
        }
        Ok(self.unmapped(here, format!("unknown custom behavior '{name}'")))
    }

    /// Lowers a mod-morph into a fork.
    ///
    /// ZMK's mod-morph and RMK's fork are the same idea: one key whose output
    /// swaps while a modifier is held. The keymap cell keeps the default output,
    /// and the fork rewrites it when the modifiers match — so the key still does
    /// the right thing on the unshifted path even if the fork table is empty.
    ///
    /// A fork matches on the trigger *action* rather than a key position, so the
    /// same default output appearing elsewhere on any layer would morph too.
    /// That ambiguity is reported rather than hidden.
    fn lower_mod_morph(
        &mut self,
        morph: &super::behaviors::ModMorph,
        here: &dyn Fn() -> String,
    ) -> Result<u16> {
        let default = morph
            .cases
            .iter()
            .find(|case| case.mods.is_empty())
            .with_context(|| format!("{} {} has no unmodified case", here(), morph.name))?;
        let morphed = match morph.cases.iter().find(|case| !case.mods.is_empty()) {
            Some(case) => case,
            None => {
                // Every case is unconditional, so there is nothing to fork on.
                return self.convert_binding(&default.binding, here);
            }
        };
        if morph.cases.len() > 2 {
            self.report(
                Severity::Approximated,
                Some(here()),
                format!(
                    "{} has {} cases; a fork carries one condition, so only the first \
                     modified case is kept",
                    morph.name,
                    morph.cases.len()
                ),
            );
        }

        let trigger = self.convert_binding(&default.binding, here)?;
        let positive = self.convert_binding(&morphed.binding, here)?;
        let match_any = zmk_state_bits(&morphed.mods)
            .with_context(|| format!("{} {} condition", here(), morph.name))?;
        let kept = zmk_modifier_combination(&morphed.keep_mods)
            .with_context(|| format!("{} {} keepMods", here(), morph.name))?;

        let fork = Fork {
            trigger: from_via_keycode(trigger),
            negative_output: from_via_keycode(trigger),
            positive_output: from_via_keycode(positive),
            match_any,
            match_none: StateBits::default(),
            kept_modifiers: kept,
            bindable: true,
        };
        if !self.forks.contains(&fork) {
            self.forks.push(fork);
        }
        // The cell holds the default output; the fork supplies the alternative.
        Ok(trigger)
    }

    /// Lowers a parameterless key-sequence macro into Rynk's macro space,
    /// returning the `TriggerMacro` keycode that runs it.
    ///
    /// Returns `None` for anything with parameters or a hold-until-release
    /// step, which the byte format has no room for.
    fn lower_macro(&mut self, entry: &super::behaviors::Macro) -> Option<u16> {
        // A parameterized macro has no fixed sequence to encode; its
        // placeholders are filled from the invoking key.
        if is_parameterized(entry) {
            return None;
        }

        // ZMK's `&macro_tap` / `&macro_press` / `&macro_release` set the mode
        // for the bindings that follow; a sequence starts out tapping.
        #[derive(Clone, Copy, PartialEq)]
        enum Mode {
            Tap,
            Press,
            Release,
        }
        let mut mode = Mode::Tap;
        let mut operations: Vec<u8> = Vec::new();
        for binding in &entry.bindings {
            match binding.name() {
                "&macro_tap" => mode = Mode::Tap,
                "&macro_press" => mode = Mode::Press,
                "&macro_release" => mode = Mode::Release,
                "&macro_pause_for_release" | "&macro_param_1to1" | "&macro_param_1to2" => {
                    return None
                }
                "&kp" => {
                    let code = binding.params.first().and_then(|param| {
                        super::zmk_keycode_param_to_via(&param.to_value(), "").ok()
                    })?;
                    let key = (code & 0xff) as u8;
                    // A modified keycode has no single-byte form, so the
                    // modifiers are pressed around the key instead.
                    let modifiers = modifier_keys((code >> 8) as u8);
                    for modifier in &modifiers {
                        operations.extend_from_slice(&[0x01, 0x02, *modifier]);
                    }
                    operations.extend_from_slice(&[
                        0x01,
                        match mode {
                            Mode::Tap => 0x01,
                            Mode::Press => 0x02,
                            Mode::Release => 0x03,
                        },
                        key,
                    ]);
                    for modifier in modifiers.iter().rev() {
                        operations.extend_from_slice(&[0x01, 0x03, *modifier]);
                    }
                }
                _ => return None,
            }
        }
        if operations.is_empty() {
            return None;
        }
        operations.push(0x00);

        let index = match self.macros.iter().position(|entry| *entry == operations) {
            Some(index) => index,
            None => {
                self.macros.push(operations);
                self.macros.len() - 1
            }
        };
        // `TriggerMacro` has five bits of index in the VIA encoding.
        (index < 32).then_some(0x7700 | index as u16)
    }

    /// Recognizes a macro that exists only to hand its own parameters to a
    /// hold-tap and hold it until release.
    ///
    /// ZMK needs the wrapper because a hold-tap node takes exactly two
    /// parameters and a key binding supplies one; the macro fans that single
    /// parameter out with `&macro_param_1to1` / `&macro_param_1to2`. The
    /// wrapper carries no behavior of its own, so lowering looks through it and
    /// returns the hold-tap with its placeholders filled in.
    fn forwarded_hold_tap(
        &self,
        entry: &super::behaviors::Macro,
        params: &[Value],
    ) -> Option<(HoldTap, Vec<Value>)> {
        // `&macro_param_AtoB` routes the invoking parameter A to placeholder B.
        let mut routes: Vec<(usize, usize)> = Vec::new();
        for binding in &entry.bindings {
            let name = binding.name();
            if let Some(rest) = name.strip_prefix("&macro_param_") {
                if let Some((from, to)) = rest.split_once("to") {
                    if let (Ok(from), Ok(to)) = (from.parse::<usize>(), to.parse::<usize>()) {
                        routes.push((from, to));
                    }
                }
                continue;
            }
            let Some(hold_tap) = self.tables.hold_tap(name) else {
                continue;
            };
            let forwarded = binding
                .params
                .iter()
                .enumerate()
                .map(|(index, param)| {
                    if param.name() != "MACRO_PLACEHOLDER" {
                        return param.value.clone();
                    }
                    routes
                        .iter()
                        .find(|(_, to)| *to == index + 1)
                        .and_then(|(from, _)| params.get(from - 1).cloned())
                        .unwrap_or(Value::Null)
                })
                .collect();
            return Some((hold_tap.clone(), forwarded));
        }
        None
    }

    fn lower_hold_tap(
        &mut self,
        hold_tap: &HoldTap,
        params: &[Value],
        here: &dyn Fn() -> String,
    ) -> Result<u16> {
        let profile = profile_of(hold_tap);

        // A hold-tap reaches a keymap cell with its two parameters already
        // bound. Which parameter is the tap is settled by the node's
        // `bindings`, not by position, so each shape reads them separately.
        //
        // Both sides are built by spelling the equivalent VIA keycode and
        // decoding it, which keeps this path on the same conversion the keymap
        // already trusts instead of assembling typed actions by hand.
        let (tap_action, hold_action) = if HrmIdiom::is_member(hold_tap) {
            // Home row mod: hold is always a plain modifier.
            let modifier = param_text(params, 0)
                .with_context(|| format!("{} {} is missing its modifier", here(), hold_tap.name))?;
            let modifier = super::zmk_modifier(&modifier)
                .with_context(|| format!("{} {} hold action", here(), hold_tap.name))?;
            let tap = plain_key(keycode_param(params, 1, here)?, hold_tap, here)?;
            tap_hold_sides(
                &format!("MT({modifier}, {})", keycodes::format_keycode(tap)),
                hold_tap,
                here,
            )?
        } else if hold_tap.hold_binding() == "&mo" {
            // Thumb layer access: hold turns on a layer, tap sends a key.
            let editor_layer = param_u8(params, 0)
                .with_context(|| format!("{} {} is missing its layer", here(), hold_tap.name))?
                as usize;
            let layer = self.remap_layer(editor_layer).with_context(|| {
                format!(
                    "{} {} targets layer {editor_layer}, which the import dropped",
                    here(),
                    hold_tap.name
                )
            })?;
            let tap = plain_key(keycode_param(params, 1, here)?, hold_tap, here)?;
            tap_hold_sides(
                &format!("LT({layer}, {})", keycodes::format_keycode(tap)),
                hold_tap,
                here,
            )?
        } else if self
            .tables
            .macro_named(hold_tap.hold_binding())
            .is_some_and(is_shift_wrapper)
        {
            // Autoshift: tap sends the key, hold sends it shifted.
            let tap = plain_key(keycode_param(params, 0, here)?, hold_tap, here)?;
            let shifted =
                keycodes::parse_keycode(&format!("LSFT({})", keycodes::format_keycode(tap)))
                    .with_context(|| format!("{} {} hold action", here(), hold_tap.name))?;
            (single_action(tap)?, single_action(shifted)?)
        } else if hold_tap.tap_binding().starts_with("&sticky_key") {
            // A modifier that arms as a one-shot when tapped and is simply held
            // when held. `&kp MOD` on the hold side is the modifier's own
            // keycode, which RMK holds for as long as the key is down.
            let hold = param_text(params, 0).with_context(|| {
                format!("{} {} is missing its hold modifier", here(), hold_tap.name)
            })?;
            let tap = param_text(params, 1).unwrap_or_else(|| hold.clone());
            let held = keycodes::parse_keycode(&super::zmk_modifier_keycode(&hold)?)
                .with_context(|| format!("{} {} hold action", here(), hold_tap.name))?;
            let armed = keycodes::parse_keycode(&format!("OSM({})", super::zmk_modifier(&tap)?))
                .with_context(|| format!("{} {} tap action", here(), hold_tap.name))?;
            (single_action(armed)?, single_action(held)?)
        } else {
            bail!(
                "{} hold-tap '{}' has no Rynk equivalent yet",
                here(),
                hold_tap.name
            );
        };

        let mut morse = Morse {
            profile,
            ..Morse::default()
        };
        let _ = morse.put(TAP, tap_action);
        let _ = morse.put(HOLD, hold_action);
        self.intern(morse)
    }

    /// Lowers the combo table.
    ///
    /// ZMK matches a combo on key *positions*; Rynk matches on the *actions*
    /// those positions hold, so each combo is resolved against the layer it is
    /// declared for. A combo listing several layers becomes one Rynk combo per
    /// layer, because `Combo::layer` takes a single index.
    pub fn lower_combos(
        &mut self,
        action_at: &dyn Fn(usize, usize) -> Option<KeyAction>,
        layer_count: usize,
        position_count: usize,
    ) -> Vec<Combo> {
        let mut out = Vec::new();
        // Rynk has one combo window for the whole keyboard, so the export's
        // per-combo timeouts can only be honoured when they agree.
        let mut windows: Vec<u32> = self
            .tables
            .combos
            .iter()
            .filter(|combo| !combo.key_positions.is_empty())
            .filter_map(|combo| combo.timeout_ms)
            .collect();
        windows.sort_unstable();
        windows.dedup();
        let shared_timeout = (windows.len() == 1).then(|| windows[0]);

        for combo in self.tables.combos.clone() {
            // A template may carry combos purely as metadata to preserve on
            // export; without trigger positions there is nothing to import.
            if combo.key_positions.is_empty() {
                continue;
            }
            let here = || format!("combo '{}'", combo.name);
            if let Some(timeout) = combo.timeout_ms {
                if Some(timeout) != shared_timeout {
                    self.report(
                        Severity::Approximated,
                        Some(here()),
                        format!(
                            "wants a {timeout} ms window, but Rynk's combo timeout is global; \
                             the shared setting applies instead"
                        ),
                    );
                }
            }
            let Some(output) = self.combo_output(&combo.binding, &here) else {
                continue;
            };

            // No `layers` key means every layer; -1 appears in exports as the
            // same "any layer" marker.
            let editor_layers: Vec<usize> = match &combo.layers {
                Some(layers) if !layers.iter().any(|layer| *layer < 0) => {
                    layers.iter().map(|layer| *layer as usize).collect()
                }
                _ => (0..layer_count).collect(),
            };

            for editor_layer in editor_layers {
                let Some(layer) = self.remap_layer(editor_layer) else {
                    continue;
                };
                let mut actions = heapless::Vec::new();
                let mut usable = true;
                for position in &combo.key_positions {
                    match action_at(editor_layer, *position) {
                        Some(action) if actions.push(action).is_ok() => {}
                        Some(_) => {
                            self.report(
                                Severity::Dropped,
                                Some(here()),
                                "has more keys than Rynk's combo length allows",
                            );
                            usable = false;
                            break;
                        }
                        None => {
                            self.report(
                                Severity::Dropped,
                                Some(here()),
                                format!("key position {position} is not readable on layer {editor_layer}"),
                            );
                            usable = false;
                            break;
                        }
                    }
                }
                if usable {
                    if let Some(extra_positions) = ambiguous_combo_positions(
                        &actions,
                        &combo.key_positions,
                        editor_layer,
                        position_count,
                        action_at,
                    ) {
                        self.report(
                            Severity::Approximated,
                            Some(here()),
                            format!(
                                "combo '{}' on editor layer {editor_layer} has the same trigger \
                                 actions at extra key positions {extra_positions:?}; Rynk may \
                                 fire it from that chord too",
                                combo.name
                            ),
                        );
                    }
                    out.push(Combo {
                        actions,
                        output,
                        layer: Some(layer),
                    });
                }
            }
        }
        out
    }

    /// A mod-morph case's output as a VIA keycode, through the same converter
    /// the keymap path uses so a nested `LS(N9)` resolves identically.
    fn convert_binding(&mut self, binding: &Binding, here: &dyn Fn() -> String) -> Result<u16> {
        match binding.name() {
            "&kp" => {
                let param = binding
                    .to_value()
                    .get("params")
                    .and_then(|params| params.as_array())
                    .and_then(|params| params.first())
                    .cloned()
                    .with_context(|| format!("{} &kp is missing its keycode", here()))?;
                super::zmk_keycode_param_to_via(&param, &here())
            }
            other => bail!("{} mod-morph case '{other}' has no Rynk equivalent", here()),
        }
    }

    fn combo_output(&mut self, binding: &Binding, here: &dyn Fn() -> String) -> Option<KeyAction> {
        match binding.name() {
            "&kp" => binding
                .param_text(0)
                .and_then(|code| super::zmk_keycode_to_via(&code).ok())
                .map(from_via_keycode),
            "&caps_word" => Some(from_via_keycode(
                crate::keycodes::parse_keycode("QK_CAPS_WORD_TOGGLE").ok()?,
            )),
            "&tog" => binding
                .param_u8(0)
                .and_then(|layer| self.remap_layer(layer as usize))
                .map(|layer| KeyAction::Single(Action::LayerToggle(layer))),
            // These three are converted a few lines away for a keymap cell; a
            // combo output is the same conversion, and leaving them out here is
            // why a layout's sticky-modifier and layer-reset chords vanished.
            "&sk" => binding
                .param_text(0)
                .and_then(|modifier| super::zmk_modifier(&modifier).ok())
                .and_then(|modifier| keycodes::parse_keycode(&format!("OSM({modifier})")).ok())
                .map(from_via_keycode),
            "&to" => binding
                .param_u8(0)
                .and_then(|layer| self.remap_layer(layer as usize))
                .map(|layer| KeyAction::Single(Action::DefaultLayer(layer))),
            "&sl" => binding
                .param_u8(0)
                .and_then(|layer| self.remap_layer(layer as usize))
                .and_then(|layer| keycodes::parse_keycode(&format!("OSL({layer})")).ok())
                .map(from_via_keycode),
            "&mo" => binding
                .param_u8(0)
                .and_then(|layer| self.remap_layer(layer as usize))
                .map(|layer| KeyAction::Single(Action::LayerOn(layer))),
            // A window-switcher chord: hold a modifier and a layer together for
            // as long as the combo keys are held, so the layer's Tab steps
            // through the list.
            name if self
                .tables
                .macro_named(name)
                .is_some_and(holds_modifier_with_layer) =>
            {
                let modifier = binding.param_text(0)?;
                let modifier = super::zmk_modifier(&modifier).ok()?;
                let layer = self.remap_layer(binding.param_u8(1)? as usize)?;
                // `LMT` is the chord exactly: hold the modifier and the layer
                // for as long as the combo keys are held, tapping the wrapped
                // key once when the chord engages.
                let tap = self
                    .tables
                    .macro_named(name)
                    .and_then(|entry| chord_tap_key(&entry.clone(), self.tables))?;
                keycodes::parse_keycode(&format!("LMT({layer}, {modifier}, {tap})"))
                    .ok()
                    .map(from_via_keycode)
            }
            other
                if self.tables.hold_tap(other).is_some()
                    || self.tables.macro_named(other).is_some()
                    || self.tables.mod_morph_named(other).is_some() =>
            {
                // A custom behavior resolves to a single keycode for a keymap
                // cell — a morse index for a hold-tap, a macro trigger — and a
                // combo output is the same kind of value, so run the same
                // resolver. `resolve` reports its own gaps and yields KC_NO,
                // which is nothing worth firing a combo for.
                let params: Vec<Value> = binding.params.iter().map(Binding::to_value).collect();
                match self.resolve(other, &params, here) {
                    Ok(0) | Err(_) => None,
                    Ok(code) => Some(from_via_keycode(code)),
                }
            }
            other => {
                // The combo is discarded right after this, so it is a drop. Any
                // other severity would let `validate` call a file clean while
                // two thirds of its combos quietly failed to arrive.
                self.report(
                    Severity::Dropped,
                    Some(here()),
                    format!("output behavior '{other}' has no Rynk equivalent"),
                );
                None
            }
        }
    }
}

fn ambiguous_combo_positions(
    trigger_actions: &[KeyAction],
    declared_positions: &[usize],
    editor_layer: usize,
    position_count: usize,
    action_at: &dyn Fn(usize, usize) -> Option<KeyAction>,
) -> Option<Vec<usize>> {
    let mut needed: Vec<(KeyAction, usize)> = Vec::new();
    for action in trigger_actions
        .iter()
        .filter(|action| !matches!(action, KeyAction::No | KeyAction::Transparent))
    {
        match needed
            .iter_mut()
            .find(|(candidate, _)| *candidate == *action)
        {
            Some((_, count)) => *count += 1,
            None => needed.push((*action, 1)),
        }
    }
    if needed.is_empty() {
        return None;
    }

    // A repeated action alone is not enough: another physical chord must be
    // able to supply the complete trigger multiset. Requiring every member of
    // this witness to be outside the declared positions also means a two-key
    // combo with one layer-unique trigger cannot be reported as ambiguous.
    let mut extra_positions = Vec::new();
    for position in (0..position_count).filter(|position| !declared_positions.contains(position)) {
        let Some(action) = action_at(editor_layer, position) else {
            continue;
        };
        if matches!(action, KeyAction::No | KeyAction::Transparent) {
            continue;
        }
        if let Some((_, count)) = needed
            .iter_mut()
            .find(|(candidate, count)| *count > 0 && *candidate == action)
        {
            *count -= 1;
            extra_positions.push(position);
        }
    }

    needed
        .iter()
        .all(|(_, count)| *count == 0)
        .then_some(extra_positions)
}

/// An editor parameter is either a bare value or a `{ "value": ... }` wrapper.
fn param_value(params: &[Value], index: usize) -> Option<&Value> {
    params.get(index).map(|param| {
        param
            .as_object()
            .and_then(|object| object.get("value"))
            .unwrap_or(param)
    })
}

fn param_text(params: &[Value], index: usize) -> Option<String> {
    param_value(params, index).map(|value| match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    })
}

fn param_u8(params: &[Value], index: usize) -> Option<u8> {
    param_value(params, index)
        .and_then(Value::as_u64)
        .and_then(|value| u8::try_from(value).ok())
}

fn keycode_param(params: &[Value], index: usize, here: &dyn Fn() -> String) -> Result<u16> {
    let value = params
        .get(index)
        .with_context(|| format!("{} is missing parameter {}", here(), index + 1))?;
    super::zmk_keycode_param_to_via(value, &here())
}

fn single_action(code: u16) -> Result<Action> {
    match from_via_keycode(code) {
        KeyAction::Single(action) => Ok(action),
        _ => bail!("only a plain key or action can sit on this side of a tap-hold"),
    }
}

/// The tap side of a tap-hold must be an unmodified HID key, because that is
/// all the `MT`/`LT` keycode encodings have room for.
fn plain_key(code: u16, hold_tap: &HoldTap, here: &dyn Fn() -> String) -> Result<u16> {
    if code > 0xff {
        bail!(
            "{} {} tap action must be an unmodified HID key",
            here(),
            hold_tap.name
        );
    }
    Ok(code)
}

/// Splits a spelled `MT`/`LT` keycode back into its (tap, hold) actions.
fn tap_hold_sides(
    spelling: &str,
    hold_tap: &HoldTap,
    here: &dyn Fn() -> String,
) -> Result<(Action, Action)> {
    let code = keycodes::parse_keycode(spelling)
        .with_context(|| format!("{} {}", here(), hold_tap.name))?;
    match from_via_keycode(code) {
        KeyAction::TapHold(tap, hold, _) => Ok((tap, hold)),
        _ => bail!("{} {} did not lower to a tap-hold", here(), hold_tap.name),
    }
}

/// Whether a macro turns on a layer *and* holds a modifier until its own key
/// is released, which is what a window-switcher chord is built from.
fn holds_modifier_with_layer(entry: &super::behaviors::Macro) -> bool {
    // Editors disagree about whether to declare `params`: TailorKey writes
    // `["code", "layer"]` while the Engrammer omits the array and leaves the
    // arity implied by its `&macro_param_*` bindings. Read the bindings, which
    // both spell the same way.
    is_parameterized(entry)
        && entry
            .bindings
            .iter()
            .any(|binding| binding.name() == "&macro_pause_for_release")
        && entry.bindings.iter().any(|binding| binding.name() == "&mo")
}

/// Whether a macro takes parameters from its invoking key.
///
/// Trusting the declared `params` alone is not safe: an export may omit it and
/// still carry `MACRO_PLACEHOLDER` bindings, which would then be encoded as a
/// literal keycode.
fn is_parameterized(entry: &super::behaviors::Macro) -> bool {
    !entry.params.is_empty()
        || entry.bindings.iter().any(|binding| {
            binding.name().starts_with("&macro_param_")
                || binding
                    .params
                    .iter()
                    .any(|param| param.name() == "MACRO_PLACEHOLDER")
        })
}

/// The key a window-switcher chord taps when it engages.
///
/// The chord macro delegates its hold to an inner macro that presses the
/// modifier and taps a fixed key (`&macro_tap`, `&kp TAB`); that key is the
/// one `LMT` must reproduce.
fn chord_tap_key(
    entry: &super::behaviors::Macro,
    tables: &super::behaviors::BehaviorTables,
) -> Option<String> {
    let inner_of = |entry: &super::behaviors::Macro| -> Option<String> {
        let mut tapping = false;
        for binding in &entry.bindings {
            match binding.name() {
                "&macro_tap" => tapping = true,
                "&macro_press" | "&macro_release" => tapping = false,
                "&kp" if tapping => {
                    let text = binding.param_text(0)?;
                    if text != "MACRO_PLACEHOLDER" {
                        return super::zmk_keycode_to_via(&text)
                            .ok()
                            .filter(|code| *code <= 0xff)
                            .map(keycodes::format_keycode);
                    }
                }
                _ => {}
            }
        }
        None
    };

    if let Some(key) = inner_of(entry) {
        return Some(key);
    }
    entry
        .bindings
        .iter()
        .filter_map(|binding| tables.macro_named(binding.name()))
        .find_map(inner_of)
}

/// Whether a macro is the "press shift, tap the parameter, release shift"
/// shape that autoshift is built from.
fn is_shift_wrapper(entry: &super::behaviors::Macro) -> bool {
    let mut presses_shift = false;
    let mut taps_param = false;
    for binding in &entry.bindings {
        match binding.name() {
            "&kp" => {
                if binding
                    .param_text(0)
                    .is_some_and(|code| code == "LSHFT" || code == "RSHFT")
                {
                    presses_shift = true;
                }
            }
            "&macro_param_1to1" => taps_param = true,
            _ => {}
        }
    }
    presses_shift && taps_param
}

/// The Rynk profile equivalent of a ZMK hold-tap node's timing fields.
///
/// `unilateral_tap` is set for every member of the home-row-mod idiom: it is
/// what replaces the node's `hold-trigger-key-positions`, which Rynk applies
/// only to profile-indexed tap-holds and never to a morse key.
fn profile_of(hold_tap: &HoldTap) -> MorseProfile {
    let mode = match hold_tap.flavor.as_deref() {
        Some("tap-preferred") => Some(MorseMode::Normal),
        Some("hold-preferred") => Some(MorseMode::HoldOnOtherPress),
        Some("balanced") => Some(MorseMode::PermissiveHold),
        Some("tap-unless-interrupted") => Some(MorseMode::TapUnlessInterrupted),
        _ => None,
    };
    MorseProfile::const_default()
        .with_mode(mode)
        .with_hold_timeout_ms(
            hold_tap
                .tapping_term_ms
                .and_then(|ms| u16::try_from(ms).ok()),
        )
        .with_quick_tap_timeout_ms(hold_tap.quick_tap_ms.and_then(|ms| u16::try_from(ms).ok()))
        .with_prior_idle_time_ms(
            hold_tap
                .require_prior_idle_ms
                .and_then(|ms| u16::try_from(ms).ok()),
        )
        .with_unilateral_tap(HrmIdiom::is_member(hold_tap).then_some(true))
        .with_hold_trigger_on_release(hold_tap.hold_trigger_on_release.then_some(true))
}
