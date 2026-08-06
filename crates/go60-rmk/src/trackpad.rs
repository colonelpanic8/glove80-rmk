//! The Go60's Cirque Pinnacle trackpad, one per half.
//!
//! Wiring and configuration are transcribed from MoErgo's official
//! `moergo-sc/zmk` Go60 board definitions: the pad sits on SPI1
//! (SCK P0.19 / MISO P0.21 / MOSI P0.22 at 1 MHz) with chip select on
//! P0.25 and the data-ready line on P0.23 (active high, pull-down), and
//! runs with 1x sensitivity, hardware 90° rotation, Y inversion, and the
//! secondary tap disabled. Both halves are wired identically; the
//! peripheral's events reach the central over the split link.
//!
//! The two pads are told apart by their RMK pointing-device id, and each
//! gets its own [`PointingProcessor`] so they can behave differently. What
//! they actually do is not decided here: RMK holds the configuration, a
//! host writes it over Rynk, and storage carries it across reboots.

use embassy_nrf::gpio::{Input, Level, Output, OutputDrive, Pull};
use embassy_nrf::spim::{self, Spim};
use embassy_nrf::{Peri, bind_interrupts, peripherals};
use rmk::input_device::cirque_pinnacle::{CirquePinnacle, PinnacleConfig, PinnacleSensitivity};
use rmk::input_device::pointing::{PointingProcessor, PointingProcessorConfig};
use rmk::keymap::KeyMap;

/// The left half runs the split central, so its pad is device 0.
pub const LEFT_DEVICE_ID: u8 = 0;
/// The right half is the split peripheral, so its pad is device 1.
pub const RIGHT_DEVICE_ID: u8 = 1;

/// A processor bound to a single pad. Without the `device_id` filter one
/// processor would answer for both pads and they could not differ.
pub fn processor<'a>(keymap: &'a KeyMap<'a>, device_id: u8) -> PointingProcessor<'a> {
    PointingProcessor::new(
        keymap,
        PointingProcessorConfig {
            device_id,
            ..Default::default()
        },
    )
}

bind_interrupts!(struct Irqs {
    TWISPI1 => spim::InterruptHandler<peripherals::TWISPI1>;
});

pub fn init(
    device_id: u8,
    spi: Peri<'static, peripherals::TWISPI1>,
    sck: Peri<'static, peripherals::P0_19>,
    miso: Peri<'static, peripherals::P0_21>,
    mosi: Peri<'static, peripherals::P0_22>,
    cs: Peri<'static, peripherals::P0_25>,
    dr: Peri<'static, peripherals::P0_23>,
) -> CirquePinnacle<Spim<'static>, Output<'static>, Input<'static>> {
    let mut spi_config = spim::Config::default();
    spi_config.frequency = spim::Frequency::M1;
    spi_config.mode = spim::MODE_1;
    let spi = Spim::new(spi, Irqs, sck, miso, mosi, spi_config);

    let cs = Output::new(cs, Level::High, OutputDrive::Standard);
    let dr = Input::new(dr, Pull::Down);

    CirquePinnacle::new(
        device_id,
        spi,
        cs,
        dr,
        PinnacleConfig {
            sensitivity: PinnacleSensitivity::X1,
            rotate_90: true,
            y_invert: true,
            no_secondary_tap: true,
            ..Default::default()
        },
    )
}
