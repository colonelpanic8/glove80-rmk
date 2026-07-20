//! Physical right-half bootloader routing.
//!
//! Split key actions are resolved on the central, so binding the right-half
//! key directly to RMK's `Bootloader` action would reboot the left half. The
//! keymap uses an otherwise-unused user action instead; this processor turns
//! its release into a request for the lighting task, which owns split state
//! and can safely dispatch the existing magic-guarded bootloader message.

use rmk::event::ActionEvent;
use rmk::types::action::Action;

/// User action reserved for the right-half physical bootloader key.
pub const PERIPHERAL_BOOTLOADER_ACTION: u8 = 12;

#[rmk::macros::processor(subscribe = [ActionEvent])]
pub struct RemoteBootloaderKey;

impl RemoteBootloaderKey {
    async fn on_action_event(&mut self, event: ActionEvent) {
        if !event.keyboard_event.pressed
            && event.action == Action::User(PERIPHERAL_BOOTLOADER_ACTION)
        {
            // A second release while one request is pending is equivalent to
            // the first. Never block the keyboard task on split traffic.
            let _ = crate::lighting::REMOTE_BOOT_REQUESTS.try_send(());
        }
    }
}
