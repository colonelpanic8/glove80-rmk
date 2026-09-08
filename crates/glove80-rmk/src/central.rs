#![no_main]
#![no_std]

pub const BOARD_LEDS_PER_HALF: usize = 40;
pub const BOARD_SCENE_CAPACITY: usize = 100;
pub const BOARD_CHANNEL_CEILING: u8 = 230;
pub const BOARD_KEEP_LED_POWER_WHILE_AWAKE: bool = false;
pub const BOARD_KEEP_LED_POWER_WHILE_SUSPENDED: bool = false;
pub const BOARD_MAINTENANCE_LED: u16 = 12;
pub const BOARD_SPLIT_TRANSPORT_LED: u16 = 12;

#[path = "../../moergo-rmk/src/central_lighting.rs"]
mod central_lighting;
#[allow(dead_code)]
#[path = "../../moergo-rmk/src/lighting.rs"]
mod lighting;
#[path = "../../moergo-rmk/src/remote_boot.rs"]
mod remote_boot;
#[allow(dead_code)]
#[path = "../../moergo-rmk/src/split_lighting.rs"]
mod split_lighting;

use rmk::macros::rmk_central;

#[rmk_central]
mod keyboard_central {
    /// Bind the macro-created Rynk transports to this board's lighting
    /// descriptor and protocol mailbox.
    #[Overwritten(host_service)]
    fn host_service() {
        use core::fmt::Write as _;

        let dirty = if env!("MOERGO_REPO_GIT_DIRTY") == "1" {
            "-dirty"
        } else {
            ""
        };
        let config_dirty = if env!("MOERGO_CONFIG_GIT_DIRTY") == "1" {
            "-dirty"
        } else {
            ""
        };
        let mut build_label = ::rmk::heapless::String::<128>::new();
        let _ = write!(
            build_label,
            "config {}{} / {} v{} ({}{}) / RMK {}",
            env!("MOERGO_CONFIG_GIT_HASH"),
            config_dirty,
            env!("CARGO_PKG_NAME"),
            env!("CARGO_PKG_VERSION"),
            env!("MOERGO_REPO_GIT_HASH"),
            dirty,
            env!("MOERGO_RMK_GIT_VERSION"),
        );

        ::rmk::host::HostService::new(&keymap, &rmk_config)
            .with_lighting(crate::central_lighting::rynk_controller())
            .with_peripheral_bootloader(crate::central_lighting::route_peripheral_bootloader)
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
            ::rmk::types::protocol::rynk::LightingAdvancedConditionalSceneCell,
            { crate::lighting::SCENE_CAPACITY },
        >::new();
        storage
            .read_lighting_runtime_conditional_scenes(&mut persisted_runtime_conditional_scenes)
            .await;
        let preferences = crate::lighting::load_preferences(&mut storage).await;
        crate::central_lighting::init(
            keymap_ref,
            persisted_scenes.as_slice(),
            persisted_policy,
            persisted_runtime_conditional_scenes.as_slice(),
            preferences,
            p.SPI3,
            p.P0_27,
            p.P0_31,
        )
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

pub fn debug_stamp(stage: u32) {
    let _ = stage;
}

pub fn debug_trace_parts() -> (u32, u32, [u32; 2], [u32; 2]) {
    (0, 0, [0; 2], [0; 2])
}

pub fn debug_panic_loc() -> Option<heapless::String<64>> {
    None
}
