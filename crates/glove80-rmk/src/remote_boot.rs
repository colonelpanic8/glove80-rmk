//! Magic-key lighting controls and physical right-half bootloader routing.
//!
//! Split key actions are resolved on the central, so binding the right-half
//! key directly to RMK's `Bootloader` action would reboot the left half. This
//! processor also maps board-reserved user actions onto the standard lighting
//! action path, keeping the runtime keymap's 16-bit representation sufficient.

use rmk::event::ActionEvent;
use rmk::types::action::{Action, LightAction};

/// User action reserved for the right-half physical bootloader key.
pub const PERIPHERAL_BOOTLOADER_ACTION: u8 = 12;
#[rmk::macros::processor(subscribe = [ActionEvent])]
pub struct MagicKeyActions;

impl MagicKeyActions {
    async fn on_action_event(&mut self, event: ActionEvent) {
        match (event.keyboard_event.pressed, event.action) {
            (false, Action::User(action))
                if crate::LIGHTING_CONTROLS.output_toggle_user_action == Some(action) =>
            {
                rmk::lighting::send_light_action(LightAction::BacklightToggle).await;
            }
            (false, Action::User(action))
                if crate::LIGHTING_CONTROLS.output_mode_cycle_user_action == Some(action) =>
            {
                rmk::lighting::send_light_action(LightAction::OutputModeCycle).await;
            }
            (false, Action::User(PERIPHERAL_BOOTLOADER_ACTION)) => {
                // A second release while one request is pending is equivalent
                // to the first. Never block the keyboard task on split traffic.
                let _ = crate::central_lighting::REMOTE_BOOT_REQUESTS.try_send(());
            }
            _ => {}
        }
    }
}
