#![no_main]
#![no_std]

mod central_lighting;
#[allow(dead_code)] // This shared module also contains the peripheral receiver.
mod lighting;
mod remote_boot;

use rmk::macros::rmk_central;

#[rmk_central]
mod keyboard_central {
    /// Bind the macro-created Rynk transports to this board's lighting
    /// descriptor and protocol mailbox.
    #[Overwritten(host_service)]
    fn host_service() {
        ::rmk::host::HostService::new(&keymap, &rmk_config)
            .with_lighting(crate::central_lighting::rynk_controller())
    }

    /// Central owner of the board-wide RMK lighting engine. Its output writes
    /// the left WS2812 chain and forwards the right frame over the split app
    /// channel.
    #[register_processor(runnable)]
    fn lighting_processor() {
        let keymap_ref = &keymap;
        crate::central_lighting::init(keymap_ref, p.SPI3, p.P0_27, p.P0_31, p.PWM0, p.P1_15)
    }

    /// Type-erased Rynk requests are translated into the standard engine's
    /// authoritative command mailbox here.
    #[register_processor(runnable)]
    fn lighting_rynk_adapter() {
        crate::central_lighting::rynk_adapter()
    }

    /// Forward the physical right-half bootloader action.
    #[register_processor(runnable)]
    fn remote_boot_dispatcher() {
        crate::central_lighting::RemoteBootDispatcher
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
