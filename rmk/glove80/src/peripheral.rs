#![no_main]
#![no_std]

// `lighting` selects on the host-request channel, so the module is compiled
// on both halves; the peripheral registers no transport pump, so the channel
// never fires here (host transports are Phase 2 central-only, and Phase 3
// split forwarding will reuse this seam).
mod host_proto;
mod lighting;

use rmk::macros::rmk_peripheral;

#[rmk_peripheral(id = 0)]
mod keyboard_peripheral {
    /// Right-half lighting (spike stage 5): WS2812 chain on SPI3 / P0.13 with
    /// chain power enable on P0.19, rear power-button LED on PWM0 / P0.16.
    /// The chain is rendered locally; the layer color still tracks the
    /// central because RMK's split peripheral republishes the synced
    /// `LayerChangeEvent` locally (no split-protocol changes involved).
    #[register_processor(event)]
    fn lighting_processor() {
        // When a custom processor is registered, the peripheral macro emits
        // `use ::rmk::core_traits::Runnable;` but our executor only names
        // `Processor`; reference the trait here so the generated import is
        // used and the build stays warning-free.
        fn _keeps_generated_runnable_import_used<T: Runnable>() {}

        crate::lighting::init(p.SPI3, p.P0_13, p.P0_19, p.PWM0, p.P0_16)
    }
}
