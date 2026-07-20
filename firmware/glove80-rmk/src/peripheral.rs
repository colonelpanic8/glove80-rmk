#![no_main]
#![no_std]

mod lighting;

use rmk::macros::rmk_peripheral;

#[rmk_peripheral(id = 0)]
mod keyboard_peripheral {
    /// Receive complete, sequence-checked right-half frames from the central
    /// board-wide lighting owner and present them to the local chain.
    #[register_processor(runnable)]
    fn lighting_processor() {
        crate::lighting::init_peripheral(p.SPI3, p.P0_13, p.P0_19, p.PWM0, p.P0_16)
    }
}
