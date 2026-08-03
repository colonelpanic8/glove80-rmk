#![no_main]
#![no_std]

pub const BOARD_LEDS_PER_HALF: usize = 30;
pub const BOARD_CHANNEL_CEILING: u8 = 102;

#[allow(dead_code)]
#[path = "../../glove80-rmk/src/lighting.rs"]
mod lighting;
#[allow(dead_code)]
#[path = "../../glove80-rmk/src/split_lighting.rs"]
mod split_lighting;

use rmk::macros::rmk_peripheral;

#[rmk_peripheral(id = 0)]
mod keyboard_peripheral {
    #[register_processor(runnable)]
    fn lighting_processor() {
        crate::lighting::init_peripheral(p.SPI3, p.P0_27, p.P1_11, p.PWM0, p.P1_15)
    }

    #[register_processor(runnable)]
    fn lighting_replication() {
        crate::lighting::peripheral_replication()
    }

    #[register_processor(runnable)]
    fn lighting_power_monitor() {
        crate::lighting::peripheral_power_monitor()
    }

    #[register_processor(event)]
    fn reactive_key_hits() {
        crate::lighting::ReactiveKeyHits::peripheral()
    }
}
