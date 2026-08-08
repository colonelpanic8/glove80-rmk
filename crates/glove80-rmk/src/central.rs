#![no_main]
#![no_std]

pub const BOARD_LEDS_PER_HALF: usize = 40;
pub const BOARD_CHANNEL_CEILING: u8 = 230;

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
        let mut persisted_scenes = ::rmk::heapless::Vec::<
            ::rmk::types::protocol::rynk::LightingSceneCell,
            { crate::lighting::SCENE_CAPACITY },
        >::new();
        let persisted_policy = storage.read_lighting_scenes(&mut persisted_scenes).await;
        let mut persisted_runtime_conditional_scenes = ::rmk::heapless::Vec::<
            ::rmk::types::protocol::rynk::LightingExtendedConditionalSceneCell,
            { crate::lighting::SCENE_CAPACITY },
        >::new();
        storage
            .read_lighting_runtime_conditional_scenes(&mut persisted_runtime_conditional_scenes)
            .await;
        let persisted_extension = storage.read_lighting_extension_state().await;
        let persisted_overlay = storage.read_lighting_extension_overlay().await;
        crate::central_lighting::init(
            keymap_ref,
            persisted_scenes.as_slice(),
            persisted_policy,
            persisted_runtime_conditional_scenes.as_slice(),
            persisted_extension,
            persisted_overlay,
            p.SPI3,
            p.P0_27,
            p.P0_31,
        )
    }

    #[register_processor(runnable)]
    fn lighting_output_activity() {
        crate::lighting::lighting_output_activity()
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

    #[register_processor(runnable)]
    fn remote_frame_bridge() {
        crate::central_lighting::remote_frame_bridge()
    }

    /// Forward the physical right-half bootloader action.
    #[register_processor(runnable)]
    fn remote_boot_dispatcher() {
        crate::central_lighting::RemoteBootDispatcher
    }

    /// Handle Magic-layer board controls: wake/toggle master lighting and
    /// route the right-half UF2 action to the peripheral.
    #[register_processor(event)]
    fn magic_key_actions() {
        crate::remote_boot::MagicKeyActions
    }

    /// Keep the information-view battery bars synchronized with both halves.
    #[register_processor(event)]
    fn battery_lighting_state() {
        crate::central_lighting::BatteryLightingState
    }

    /// Refresh the Magic+R status LED immediately after its gate changes.
    #[register_processor(event)]
    fn maintenance_lighting_state() {
        crate::lighting::MaintenanceLightingState
    }

    /// Report this half's VBUS-derived charge state; without it no charge
    /// state is ever produced and `charge`-gated lighting rules never fire.
    #[register_processor(runnable)]
    fn lighting_power_monitor() {
        crate::lighting::power_monitor(p.PWM0, p.P1_15)
    }

    /// Feed every key press to the local PaletteFx engine and mirror left-half
    /// hits to the peripheral, allowing spatial key effects to span the seam.
    #[register_processor(event)]
    fn reactive_key_hits() {
        crate::lighting::ReactiveKeyHits::central()
    }
}
