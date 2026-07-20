#![no_main]
#![no_std]

mod config_store;
mod host_proto;
mod host_pump;
mod lighting;
mod lighting_config;
mod remote_boot;
mod split_lighting;
mod version;

use rmk::macros::rmk_central;

#[rmk_central]
mod keyboard_central {
    /// Left-half lighting (spike stage 5): WS2812 chain on SPI3 / P0.27 with
    /// chain power enable on P0.31, rear power-button LED on PWM0 / P1.15.
    /// The body runs inside the generated `main` (where `p` holds the
    /// embassy-nrf peripherals); the returned processor's event loop is
    /// joined with the other RMK tasks. As the split central it also owns
    /// the authoritative right-half overlay store and forwards lighting to
    /// the peripheral (Phase 3, `split_lighting.rs`).
    #[register_processor(event)]
    fn lighting_processor() {
        crate::lighting::init(
            p.SPI3,
            p.P0_27,
            p.P0_31,
            p.PWM0,
            p.P1_15,
            crate::split_lighting::SplitRole::central(),
        )
    }

    /// Host-protocol transport pumps (Phase 2, central only): reassemble and
    /// decode protocol messages from the USB vendor raw-HID interface and the
    /// custom GATT service, hand them to the lighting task (which owns the
    /// compositor), and frame the responses back out. Its overridden
    /// `process_loop` never subscribes to events, so no `layer_change`
    /// subscriber slot is consumed.
    #[register_processor(event)]
    fn host_transport_pump() {
        crate::host_pump::TransportPump
    }

    /// Route the Magic-layer key on the right half to that half's UF2
    /// bootloader. RMK resolves split key actions on the central, so this
    /// user action must be forwarded explicitly instead of using the local
    /// `Bootloader` action.
    #[register_processor(event)]
    fn remote_bootloader_key() {
        crate::remote_boot::RemoteBootloaderKey
    }
}
