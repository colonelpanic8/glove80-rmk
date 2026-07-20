#![no_main]
#![no_std]

// `lighting` selects on the host-request channel, so the module is compiled
// on both halves; the peripheral registers no transport pump, so the channel
// never fires here (host transports are central-only). Split lighting
// forwarding (Phase 3) arrives instead through RMK's split app channel,
// applied by the lighting task via `split_lighting.rs`.
mod config_store;
mod host_proto;
mod lighting;
mod lighting_config;
mod split_lighting;
mod version;

use rmk::macros::rmk_peripheral;

#[rmk_peripheral(id = 0)]
mod keyboard_peripheral {
    /// Right-half lighting (spike stage 5): WS2812 chain on SPI3 / P0.13 with
    /// chain power enable on P0.19, rear power-button LED on PWM0 / P0.16.
    /// The chain is rendered locally; the layer color still tracks the
    /// central because RMK's split peripheral republishes the synced
    /// `LayerChangeEvent` locally. Host-overlay cells for this half arrive
    /// over the split app pipe (Phase 3) and are applied by the lighting
    /// task, which stays the compositor's single owner.
    #[register_processor(event)]
    fn lighting_processor() {
        // When a custom processor is registered, the peripheral macro emits
        // `use ::rmk::core_traits::Runnable;` but our executor only names
        // `Processor`; reference the trait here so the generated import is
        // used and the build stays warning-free.
        fn _keeps_generated_runnable_import_used<T: Runnable>() {}

        crate::lighting::init(
            p.SPI3,
            p.P0_13,
            p.P0_19,
            p.PWM0,
            p.P0_16,
            crate::split_lighting::SplitRole::peripheral(),
        )
    }
}
