#![no_main]
#![no_std]

mod lighting;

use rmk::macros::rmk_central;

#[rmk_central]
mod keyboard_central {
    /// Left-half lighting (spike stage 5): WS2812 chain on SPI3 / P0.27 with
    /// chain power enable on P0.31, rear power-button LED on PWM0 / P1.15.
    /// The body runs inside the generated `main` (where `p` holds the
    /// embassy-nrf peripherals); the returned processor's event loop is
    /// joined with the other RMK tasks.
    #[register_processor(event)]
    fn lighting_processor() {
        crate::lighting::init(p.SPI3, p.P0_27, p.P0_31, p.PWM0, p.P1_15)
    }
}
