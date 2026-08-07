//! `connection` verb: read and drive the keyboard's transports and BLE slots.
//!
//! Everything here rides the management protocol, so it works regardless of
//! which transport currently carries typing — including switching the typing
//! target itself.

use anyhow::Result;
use clap::Subcommand;
use rynk::rmk_types::ble::BleState;
use rynk::rmk_types::connection::{ConnectionStatus, ConnectionType, UsbState};

use crate::transport::Selector;

#[derive(Subcommand)]
pub enum ConnectionCommand {
    /// Show the transports, the selected BLE slot, and where typing goes.
    Status,
    /// Select a BLE slot; an empty slot starts advertising for pairing.
    Switch { slot: u8 },
    /// Forget the bond stored in a BLE slot.
    Clear { slot: u8 },
    /// Read or replace the BLE advertising-name template.
    Name {
        #[command(subcommand)]
        command: NameCommand,
    },
}

#[derive(Subcommand)]
pub enum NameCommand {
    /// Show the persistent name template.
    Get,
    /// Replace the template; `{slot}` expands to the one-based active slot.
    Set { template: String },
}

pub fn run(selector: &Selector, command: &ConnectionCommand) -> Result<()> {
    crate::rynk_client::run_connection(selector, command)
}

pub(crate) fn render(status: &ConnectionStatus) -> String {
    let usb = match status.usb {
        UsbState::Disabled => "disabled",
        UsbState::Enabled => "enabled (not configured)",
        UsbState::Configured => "connected",
        UsbState::Suspended => "suspended",
    };
    let ble = match status.ble.state {
        BleState::Advertising => "advertising (pairable)",
        BleState::Connected => "connected",
        BleState::Inactive => "inactive",
    };
    let preferred = match status.preferred {
        ConnectionType::Usb => "usb",
        ConnectionType::Ble => "ble",
    };
    let active = match status.decide_active() {
        Some(ConnectionType::Usb) => "usb",
        Some(ConnectionType::Ble) => "ble",
        None => "none",
    };
    format!(
        "usb: {usb}\n\
         ble: slot {} {ble}\n\
         preferred: {preferred}\n\
         typing goes to: {active}\n",
        status.ble.profile,
    )
}
