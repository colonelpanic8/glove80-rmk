#![no_main]
#![no_std]

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
        crate::lighting::init_peripheral(p.SPI3, p.P0_13, p.P0_19, p.PWM0, p.P0_16)
    }

    /// Stage and atomically apply semantic snapshots from the central.
    #[register_processor(runnable)]
    fn lighting_replication() {
        crate::lighting::peripheral_replication()
    }
}
