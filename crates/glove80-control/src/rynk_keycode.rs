//! Compatibility conversion between the CLI's QMK/VIA `u16` keycode text
//! format and Rynk's native typed `KeyAction` values.
//!
//! This mirrors RMK's Vial conversion at the migration boundary. Once the
//! canonical config format stores native Rynk actions, this module can go away.

use rynk::rmk_types::action::{Action, KeyAction, KeyboardAction};
use rynk::rmk_types::keycode::{HidKeyCode, KeyCode, SpecialKey};
use rynk::rmk_types::modifier::ModifierCombination;

pub fn to_via_keycode(key_action: KeyAction) -> u16 {
    match key_action {
        KeyAction::No => 0x0000,
        KeyAction::Transparent => 0x0001,
        KeyAction::Single(action) => match action {
            Action::Key(KeyCode::Hid(key)) => key as u16,
            Action::Key(KeyCode::Consumer(key)) => key.to_hid_keycode().map_or(0, |key| key as u16),
            Action::Key(KeyCode::SystemControl(key)) => {
                key.to_hid_keycode().map_or(0, |key| key as u16)
            }
            Action::Key(_) => 0,
            Action::KeyWithModifier(key, modifiers) => {
                ((modifiers.into_packed_bits() as u16) << 8) | key as u16
            }
            Action::LayerToggleOnly(layer) => 0x5200 | layer as u16,
            Action::LayerOn(layer) => 0x5220 | layer as u16,
            Action::DefaultLayer(layer) => 0x5240 | layer as u16,
            Action::PersistentDefaultLayer(layer) => 0x52E0 | layer as u16,
            Action::LayerToggle(layer) => 0x5260 | layer as u16,
            Action::TriLayerLower => 0x7c77,
            Action::TriLayerUpper => 0x7c78,
            Action::TriggerMacro(index) => 0x7700 | index as u16,
            Action::OneShotLayer(layer) if layer < 16 => 0x5280 | layer as u16,
            Action::OneShotLayer(_) => 0,
            Action::OneShotModifier(modifiers) => 0x52A0 | modifiers.into_packed_bits() as u16,
            Action::LayerOnWithModifier(layer, modifiers) if layer < 16 => {
                0x5000 | ((layer as u16) << 5) | (modifiers.into_packed_bits() & 0b1_1111) as u16
            }
            Action::LayerOnWithModifier(_, _) => 0,
            Action::KeyboardControl(action) => match action {
                KeyboardAction::Bootloader => 0x7c00,
                KeyboardAction::Reboot => 0x7c01,
                KeyboardAction::DebugToggle => 0x7c02,
                KeyboardAction::ClearEeprom => 0x7c03,
                KeyboardAction::OutputAuto => 0x7780,
                KeyboardAction::OutputUsb => 0x7784,
                KeyboardAction::OutputBluetooth => 0x7786,
                KeyboardAction::ComboOn => 0x7c50,
                KeyboardAction::ComboOff => 0x7c51,
                KeyboardAction::ComboToggle => 0x7c52,
                KeyboardAction::CapsWordToggle => 0x7c73,
                _ => 0,
            },
            Action::Special(SpecialKey::GraveEscape) => 0x7c16,
            Action::Special(SpecialKey::Repeat) => 0x7c79,
            Action::Special(_) => 0,
            Action::User(id) => (id as u16 & 0x1f) | 0x7e00,
            _ => 0,
        },
        KeyAction::Tap(_) => 0,
        KeyAction::TapHold(tap, hold, _) => match hold {
            Action::LayerOn(layer) if layer <= 16 => {
                let key = match tap {
                    Action::Key(KeyCode::Hid(key)) => key as u16,
                    _ => 0,
                };
                0x4000 | ((layer as u16) << 8) | key
            }
            Action::Modifier(modifiers) => match tap {
                Action::KeyWithModifier(key, shift) if shift == ModifierCombination::LSHIFT => {
                    match (key, modifiers) {
                        (HidKeyCode::Kc9, ModifierCombination::LCTRL) => 0x7c18,
                        (HidKeyCode::Kc0, ModifierCombination::RCTRL) => 0x7c19,
                        (HidKeyCode::Kc9, ModifierCombination::LSHIFT) => 0x7c1a,
                        (HidKeyCode::Kc0, ModifierCombination::RSHIFT) => 0x7c1b,
                        (HidKeyCode::Kc9, ModifierCombination::LALT) => 0x7c1c,
                        (HidKeyCode::Kc0, ModifierCombination::RALT) => 0x7c1d,
                        _ => 0,
                    }
                }
                Action::Key(KeyCode::Hid(key)) => {
                    0x2000 | ((modifiers.into_packed_bits() as u16) << 8) | key as u16
                }
                _ => 0,
            },
            _ => 0,
        },
        KeyAction::Morse(index) => 0x5700 | index as u16,
        _ => 0,
    }
}

pub fn from_via_keycode(code: u16) -> KeyAction {
    match code {
        0x0000 => KeyAction::No,
        0x0001 => KeyAction::Transparent,
        0x0002..=0x00ff => KeyAction::Single(Action::Key(KeyCode::Hid((code as u8).into()))),
        0x0100..=0x1fff => KeyAction::Single(Action::KeyWithModifier(
            (code as u8).into(),
            ModifierCombination::from_packed_bits((code >> 8) as u8),
        )),
        0x2000..=0x3fff => KeyAction::TapHold(
            Action::Key(KeyCode::Hid((code as u8).into())),
            Action::Modifier(ModifierCombination::from_packed_bits(
                ((code >> 8) & 0b1_1111) as u8,
            )),
            u8::MAX,
        ),
        0x4000..=0x4fff => KeyAction::TapHold(
            Action::Key(KeyCode::Hid((code as u8).into())),
            Action::LayerOn(((code >> 8) & 0xf) as u8),
            u8::MAX,
        ),
        0x5000..=0x51ff => KeyAction::Single(Action::LayerOnWithModifier(
            ((code >> 5) & 0xf) as u8,
            ModifierCombination::from_packed_bits((code & 0b1_1111) as u8),
        )),
        0x5200..=0x521f => KeyAction::Single(Action::LayerToggleOnly(code as u8 & 0xf)),
        0x5220..=0x523f => KeyAction::Single(Action::LayerOn(code as u8 & 0xf)),
        0x5240..=0x525f => KeyAction::Single(Action::DefaultLayer(code as u8 & 0xf)),
        0x5260..=0x527f => KeyAction::Single(Action::LayerToggle(code as u8 & 0xf)),
        0x5280..=0x529f => KeyAction::Single(Action::OneShotLayer(code as u8 & 0xf)),
        0x52a0..=0x52bf => KeyAction::Single(Action::OneShotModifier(
            ModifierCombination::from_packed_bits((code & 0x1f) as u8),
        )),
        0x52c0..=0x52df => KeyAction::No,
        0x52e0..=0x52ff => KeyAction::Single(Action::PersistentDefaultLayer(code as u8 & 0xf)),
        0x5700..=0x57ff => KeyAction::Morse((code & 0xff) as u8),
        0x7000..=0x701f | 0x7800..=0x783f => KeyAction::No,
        0x7700..=0x771f => KeyAction::Single(Action::TriggerMacro(code as u8 & 0x1f)),
        0x7780 => KeyAction::Single(Action::KeyboardControl(KeyboardAction::OutputAuto)),
        0x7784 => KeyAction::Single(Action::KeyboardControl(KeyboardAction::OutputUsb)),
        0x7786 => KeyAction::Single(Action::KeyboardControl(KeyboardAction::OutputBluetooth)),
        0x7c00 => KeyAction::Single(Action::KeyboardControl(KeyboardAction::Bootloader)),
        0x7c01 => KeyAction::Single(Action::KeyboardControl(KeyboardAction::Reboot)),
        0x7c02 => KeyAction::Single(Action::KeyboardControl(KeyboardAction::DebugToggle)),
        0x7c03 => KeyAction::Single(Action::KeyboardControl(KeyboardAction::ClearEeprom)),
        0x7c16 => KeyAction::Single(Action::Special(SpecialKey::GraveEscape)),
        0x7c18 => space_cadet(HidKeyCode::Kc9, ModifierCombination::LCTRL),
        0x7c19 => space_cadet(HidKeyCode::Kc0, ModifierCombination::RCTRL),
        0x7c1a => space_cadet(HidKeyCode::Kc9, ModifierCombination::LSHIFT),
        0x7c1b => space_cadet(HidKeyCode::Kc0, ModifierCombination::RSHIFT),
        0x7c1c => space_cadet(HidKeyCode::Kc9, ModifierCombination::LALT),
        0x7c1d => space_cadet(HidKeyCode::Kc0, ModifierCombination::RALT),
        0x7c1e => KeyAction::TapHold(
            Action::Key(KeyCode::Hid(HidKeyCode::Enter)),
            Action::Modifier(ModifierCombination::RSHIFT),
            u8::MAX,
        ),
        0x7c50 => KeyAction::Single(Action::KeyboardControl(KeyboardAction::ComboOn)),
        0x7c51 => KeyAction::Single(Action::KeyboardControl(KeyboardAction::ComboOff)),
        0x7c52 => KeyAction::Single(Action::KeyboardControl(KeyboardAction::ComboToggle)),
        0x7c73 => KeyAction::Single(Action::KeyboardControl(KeyboardAction::CapsWordToggle)),
        0x7c77 => KeyAction::Single(Action::TriLayerLower),
        0x7c78 => KeyAction::Single(Action::TriLayerUpper),
        0x7c79 => KeyAction::Single(Action::Special(SpecialKey::Repeat)),
        0x7c02..=0x7c5f => KeyAction::No,
        0x7e00..=0x7e1f => KeyAction::Single(Action::User(code as u8 & 0x1f)),
        _ => KeyAction::No,
    }
}

fn space_cadet(key: HidKeyCode, hold: ModifierCombination) -> KeyAction {
    KeyAction::TapHold(
        Action::KeyWithModifier(key, ModifierCombination::LSHIFT),
        Action::Modifier(hold),
        u8::MAX,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn representative_via_actions_round_trip() {
        for code in [
            0x0000, 0x0001, 0x0004, 0x0104, 0x2104, 0x4304, 0x5223, 0x5264, 0x5283, 0x52a2, 0x52e3,
            0x5702, 0x7704, 0x7780, 0x7784, 0x7786, 0x7c00, 0x7c02, 0x7c18, 0x7c79, 0x7e10,
        ] {
            assert_eq!(to_via_keycode(from_via_keycode(code)), code, "0x{code:04x}");
        }
    }
}
