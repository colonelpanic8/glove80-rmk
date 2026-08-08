#![no_main]
#![no_std]

pub const BOARD_LEDS_PER_HALF: usize = 40;
pub const BOARD_CHANNEL_CEILING: u8 = 230;

#[allow(dead_code)] // Shared with the central binary's half-specific constructors.
mod lighting;
#[allow(dead_code)] // Shared codec also contains the central snapshot sender.
mod split_lighting;

use rmk::macros::rmk_peripheral;

#[rmk_peripheral(id = 0)]
mod keyboard_peripheral {
    /// Render the board-wide declarative model locally and present only the
    /// right half's stable slots to its physical chain.
    #[register_processor(runnable)]
    fn lighting_processor() {
        crate::lighting::init_peripheral(p.SPI3, p.P0_13, p.P0_19)
    }

    #[register_processor(runnable)]
    fn lighting_output_activity() {
        crate::lighting::lighting_output_activity()
    }

    /// Render the native priority layer edge without waiting for bulk
    /// application traffic.
    #[register_processor(event)]
    fn fast_layer_lighting() {
        crate::lighting::FastPeripheralLayerLighting
    }

    /// Stage and atomically apply semantic snapshots from the central.
    #[register_processor(runnable)]
    fn lighting_replication() {
        crate::lighting::peripheral_replication()
    }

    #[register_processor(runnable)]
    fn lighting_replication_worker() {
        crate::lighting::peripheral_lighting_worker()
    }

    /// Re-render when this half's own USB/VBUS power changes, and report the
    /// VBUS-derived charge state.
    #[register_processor(runnable)]
    fn lighting_power_monitor() {
        crate::lighting::power_monitor(p.PWM0, p.P0_16)
    }

    /// Feed right-half presses directly to PaletteFx. Left-half presses arrive
    /// through the lighting replication task so spatial effects span both
    /// halves without double-counting local hits.
    #[register_processor(event)]
    fn reactive_key_hits() {
        crate::lighting::ReactiveKeyHits::peripheral()
    }
}
