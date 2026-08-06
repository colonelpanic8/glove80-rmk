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
//! gets its own [`PointingProcessor`] so they can behave differently: the
//! left pad scrolls, the right pad moves the cursor.

use embassy_nrf::gpio::{Input, Level, Output, OutputDrive, Pull};
use embassy_nrf::spim::{self, Spim};
use embassy_nrf::{Peri, bind_interrupts, peripherals};
use rmk::event::{LayerChangeEvent, PointingProcessorEvent, publish_event};
use rmk::input_device::cirque_pinnacle::{CirquePinnacle, PinnacleConfig, PinnacleSensitivity};
use rmk::input_device::pointing::{
    CursorConfig, DragConfig, PointingMode, PointingProcessor, PointingProcessorConfig,
    ScrollConfig,
};
use rmk::keymap::KeyMap;

/// The left half runs the split central, so its pad is device 0.
pub const LEFT_DEVICE_ID: u8 = 0;
/// The right half is the split peripheral, so its pad is device 1.
pub const RIGHT_DEVICE_ID: u8 = 1;

/// Both pads, in the order layer changes are announced to them.
pub const DEVICE_IDS: [u8; 2] = [LEFT_DEVICE_ID, RIGHT_DEVICE_ID];

/// Held from the left thumb; the keymap's navigation layer.
const SYMBOL_NAV_LAYER: u8 = 2;

/// The mode a pad starts in at boot.
pub fn default_mode(device_id: u8) -> PointingMode {
    match device_id {
        LEFT_DEVICE_ID => PointingMode::Scroll(ScrollConfig::default()),
        _ => PointingMode::Cursor(CursorConfig::default()),
    }
}

/// The mode a pad uses while `layer` is the topmost active layer. Layers
/// absent from this table leave both pads at [`default_mode`].
pub fn mode_for_layer(device_id: u8, layer: u8) -> PointingMode {
    match (layer, device_id) {
        // Symbol Nav is held on the left thumb, which turns both pads into
        // pointing tools for as long as it is down: the left one moves the
        // cursor, and the right one drags whatever its tap grabs.
        (SYMBOL_NAV_LAYER, LEFT_DEVICE_ID) => PointingMode::Cursor(CursorConfig::default()),
        (SYMBOL_NAV_LAYER, RIGHT_DEVICE_ID) => PointingMode::Drag(DragConfig::default()),
        _ => default_mode(device_id),
    }
}

/// Re-points both pads whenever the active layer changes.
///
/// `LayerChangeEvent` carries the topmost active layer, so releasing a
/// momentary layer key announces the layer underneath and restores the
/// pads without any separate bookkeeping.
#[rmk::macros::processor(subscribe = [LayerChangeEvent])]
pub struct LayerModes;

impl LayerModes {
    async fn on_layer_change_event(&mut self, LayerChangeEvent(layer): LayerChangeEvent) {
        for device_id in DEVICE_IDS {
            publish_event(PointingProcessorEvent {
                device_id,
                mode: mode_for_layer(device_id, layer),
            });
        }
    }
}

/// A processor bound to a single pad. Without the `device_id` filter one
/// processor would answer for both pads and they could not differ.
pub fn processor<'a>(keymap: &'a KeyMap<'a>, device_id: u8) -> PointingProcessor<'a> {
    let mut processor = PointingProcessor::new(
        keymap,
        PointingProcessorConfig {
            device_id,
            ..Default::default()
        },
    );
    processor.set_pointing_mode(default_mode(device_id));
    processor
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
