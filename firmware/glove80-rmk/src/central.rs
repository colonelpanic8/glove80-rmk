#![no_main]
#![no_std]

mod central_lighting;
#[allow(dead_code)] // This shared module also contains the peripheral receiver.
mod lighting;
mod remote_boot;
mod split_lighting;

use rmk::macros::rmk_central;

fn route_peripheral_bootloader(slot: u8) -> Result<(), rmk::types::protocol::rynk::RynkError> {
    if slot != 0 {
        return Err(rmk::types::protocol::rynk::RynkError::Invalid);
    }
    crate::central_lighting::REMOTE_BOOT_REQUESTS
        .try_send(())
        .map_err(|_| rmk::types::protocol::rynk::RynkError::NotReady)
}

#[rmk_central]
mod keyboard_central {
    /// Bind the macro-created Rynk transports to this board's lighting
    /// descriptor and protocol mailbox.
    #[Overwritten(host_service)]
    fn host_service() {
        use core::fmt::Write as _;

        let dirty = if env!("GLOVE80_GIT_DIRTY") == "1" {
            "-dirty"
        } else {
            ""
        };
        let config_dirty = if env!("GLOVE80_CONFIG_GIT_DIRTY") == "1" {
            "-dirty"
        } else {
            ""
        };
        let mut build_label = ::rmk::heapless::String::<128>::new();
        let _ = write!(
            build_label,
            "config {}{} / {} v{} ({}{}) / RMK {}",
            env!("GLOVE80_CONFIG_GIT_HASH"),
            config_dirty,
            env!("CARGO_PKG_NAME"),
            env!("CARGO_PKG_VERSION"),
            env!("GLOVE80_GIT_HASH"),
            dirty,
            env!("GLOVE80_RMK_GIT_VERSION"),
        );

        ::rmk::host::HostService::new(&keymap, &rmk_config)
            .with_lighting(crate::central_lighting::rynk_controller())
            .with_peripheral_bootloader(crate::route_peripheral_bootloader)
            .with_build_label(build_label.as_str())
    }

    /// Central authority and left-half renderer for the board-wide lighting
    /// model. The peripheral receives declarative snapshots separately.
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

    /// Replicate semantic state on mutations and reconnect; animation frames
    /// never traverse the split link.
    #[register_processor(runnable)]
    fn lighting_replication() {
        crate::central_lighting::replication()
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
