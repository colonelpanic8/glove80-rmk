#![no_main]
#![no_std]

pub const BOARD_LEDS_PER_HALF: usize = 30;
// MoErgo warns that Go60 RGB above 40% can exceed 500 mA.
pub const BOARD_CHANNEL_CEILING: u8 = 102;

#[path = "../../glove80-rmk/src/central_lighting.rs"]
mod central_lighting;
#[allow(dead_code)]
#[path = "../../glove80-rmk/src/lighting.rs"]
mod lighting;
#[path = "../../glove80-rmk/src/remote_boot.rs"]
mod remote_boot;
#[path = "../../glove80-rmk/src/split_lighting.rs"]
mod split_lighting;
mod trackpad;

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

    #[register_processor(runnable)]
    fn lighting_processor() {
        let mut persisted_scenes = ::rmk::heapless::Vec::<
            ::rmk::types::protocol::rynk::LightingSceneCell,
            { crate::lighting::SCENE_CAPACITY },
        >::new();
        let persisted_policy = storage.read_lighting_scenes(&mut persisted_scenes).await;
        let mut persisted_conditional_scenes = ::rmk::heapless::Vec::<
            ::rmk::types::protocol::rynk::LightingExtendedConditionalSceneCell,
            { crate::lighting::SCENE_CAPACITY },
        >::new();
        storage
            .read_lighting_runtime_conditional_scenes(&mut persisted_conditional_scenes)
            .await;
        crate::central_lighting::init(
            &keymap,
            persisted_scenes.as_slice(),
            persisted_policy,
            persisted_conditional_scenes.as_slice(),
            storage.read_lighting_extension_state().await,
            storage.read_lighting_extension_overlay().await,
            p.SPI3,
            p.P0_27,
            p.P1_11,
        )
    }

    #[register_processor(runnable)]
    fn lighting_output_activity() {
        crate::lighting::lighting_output_activity()
    }

    #[register_processor(runnable)]
    fn lighting_rynk_adapter() {
        crate::central_lighting::rynk_adapter()
    }

    #[register_processor(runnable)]
    fn lighting_replication() {
        crate::central_lighting::replication()
    }

    #[register_processor(runnable)]
    fn remote_frame_bridge() {
        crate::central_lighting::remote_frame_bridge()
    }

    #[register_processor(runnable)]
    fn remote_boot_dispatcher() {
        crate::central_lighting::RemoteBootDispatcher
    }

    #[register_processor(runnable)]
    fn trackpad_device() {
        crate::trackpad::init(
            crate::trackpad::LEFT_DEVICE_ID,
            p.TWISPI1,
            p.P0_19,
            p.P0_21,
            p.P0_22,
            p.P0_25,
            p.P0_23,
        )
    }

    #[register_processor(event)]
    fn left_pointing_processor() {
        crate::trackpad::processor(&keymap, crate::trackpad::LEFT_DEVICE_ID)
    }

    #[register_processor(event)]
    fn right_pointing_processor() {
        crate::trackpad::processor(&keymap, crate::trackpad::RIGHT_DEVICE_ID)
    }

    #[register_processor(event)]
    fn trackpad_layer_modes() {
        // Seeding from storage here is what makes the pads' behavior
        // configuration rather than firmware: nothing about them is decided
        // until this is read back.
        ::rmk::input_device::pointing_config::init(storage.read_pointing_config().await).await;
        ::rmk::input_device::pointing_config::PointingLayerModes
    }

    #[register_processor(event)]
    fn magic_key_actions() {
        crate::remote_boot::MagicKeyActions
    }

    #[register_processor(event)]
    fn battery_lighting_state() {
        crate::central_lighting::BatteryLightingState
    }

    /// Report this half's VBUS-derived charge state; without it no charge
    /// state is ever produced and `charge`-gated lighting rules never fire.
    #[register_processor(runnable)]
    fn lighting_power_monitor() {
        crate::lighting::power_monitor(p.PWM0, p.P1_15)
    }

    #[register_processor(event)]
    fn reactive_key_hits() {
        crate::lighting::ReactiveKeyHits::central()
    }
}
