//! Central ownership of the board-wide RMK lighting engine and Rynk state.
//!
//! The central is the configuration/control authority. Both halves render
//! board-wide declarative state locally; this module mirrors atomic semantic
//! snapshots rather than streaming sampled right-half RGB frames.

use embassy_futures::select::{Either, Either4, select, select4};
use embassy_nrf::Peri;
use embassy_nrf::gpio::Pin;
use embassy_nrf::peripherals::{PWM0, SPI3};
use rmk::core_traits::Runnable;
use rmk::event::{
    BatteryStatusEvent, EventSubscriber, LayerChangeEvent, LedIndicatorEvent, LightingChangedEvent,
    PeripheralBatteryEvent, SubscribableEvent,
};
use rmk::host::{
    RynkLightingController, RynkLightingDescriptor, RynkLightingMailbox,
    StandardRynkLightingAdapter, install_lighting_runtime_conditional_scenes,
    install_lighting_scenes,
};
use rmk::keymap::KeyMap;
use rmk::lighting::{
    KeymapLightingState, LightingProcessor, LightingService, LogicalFrame, Rgb8, StandardCommand,
    StandardReplicaState,
};
use rmk::split_app::SplitAppData;
use rmk::types::protocol::rynk::{
    LightingConditionalSceneCell, LightingLayerPolicy, LightingSceneCell,
};

use crate::lighting::{
    BOOTLOADER_TAG, COMMAND_CAPACITY, CORE_MAILBOX, Engine, HalfOutput, LightingHardware,
    OVERLAY_CAPACITY, REPLICA_SLOT, SCENE_CAPACITY,
};

static RYNK_MAILBOX: RynkLightingMailbox = RynkLightingMailbox::new();

/// Coalescing request from the resolved right-half bootloader key.
pub static REMOTE_BOOT_REQUESTS: embassy_sync::channel::Channel<rmk::RawMutex, (), 1> =
    embassy_sync::channel::Channel::new();

#[allow(clippy::too_many_arguments)]
pub fn init<'keymap, 'data>(
    keymap: &'keymap KeyMap<'data>,
    persisted_scenes: &[LightingSceneCell],
    persisted_policy: Option<LightingLayerPolicy>,
    persisted_runtime_conditional_scenes: &[LightingConditionalSceneCell],
    persisted_extension: Option<::rmk::storage::LightingExtensionRecord>,
    persisted_overlay: Option<::rmk::storage::LightingExtensionOverlayRecord>,
    spi: Peri<'static, SPI3>,
    data_pin: Peri<'static, impl Pin>,
    chain_power_pin: Peri<'static, impl Pin>,
    pwm: Peri<'static, PWM0>,
    status_led_pin: Peri<'static, impl Pin>,
) -> LightingProcessor<
    'static,
    KeymapLightingState<'keymap, 'data>,
    Engine,
    HalfOutput,
    COMMAND_CAPACITY,
> {
    let provider =
        KeymapLightingState::new(keymap).expect("Glove80 layer count fits lighting state");
    let mut engine = crate::lighting::engine(persisted_extension, persisted_overlay);
    install_lighting_scenes(
        &mut engine,
        &crate::LIGHTING_TOPOLOGY,
        persisted_scenes,
        persisted_policy,
    );
    install_lighting_runtime_conditional_scenes(
        &mut engine,
        &crate::LIGHTING_TOPOLOGY,
        persisted_runtime_conditional_scenes,
    );
    let service = LightingService::new(provider, engine, LogicalFrame::new(Rgb8::BLACK));
    let output = HalfOutput::left(LightingHardware::new(
        spi,
        data_pin,
        chain_power_pin,
        pwm,
        status_led_pin,
    ));
    LightingProcessor::new(service, output, &CORE_MAILBOX)
}

pub fn rynk_adapter()
-> StandardRynkLightingAdapter<'static, OVERLAY_CAPACITY, COMMAND_CAPACITY, SCENE_CAPACITY> {
    StandardRynkLightingAdapter::new(&RYNK_MAILBOX, &CORE_MAILBOX, crate::LIGHTING_TOPOLOGY)
}

pub const fn rynk_controller() -> RynkLightingController<'static> {
    RynkLightingController::new(
        &RYNK_MAILBOX,
        RynkLightingDescriptor {
            topology_revision: crate::LIGHTING_TOPOLOGY_REVISION,
            topology: crate::LIGHTING_TOPOLOGY,
            routing: crate::LIGHTING_ROUTING,
        },
        OVERLAY_CAPACITY as u16,
    )
    .with_scene_capacity(SCENE_CAPACITY as u16)
    .with_runtime_conditional_scene_capacity(SCENE_CAPACITY as u16)
    .with_conditional_scenes(&crate::LIGHTING_CONDITIONAL_SCENE_CELLS)
    .with_controls(crate::LIGHTING_CONTROLS)
    .with_extension_effects()
    .with_extension_layering()
}

/// Latch live battery state for the status source and request a fresh render.
#[rmk::macros::processor(subscribe = [BatteryStatusEvent, PeripheralBatteryEvent])]
pub struct BatteryLightingState;

impl BatteryLightingState {
    async fn on_battery_status_event(&mut self, event: BatteryStatusEvent) {
        crate::lighting::set_left_battery(event.0);
        CORE_MAILBOX.snapshot_changed();
    }

    async fn on_peripheral_battery_event(&mut self, event: PeripheralBatteryEvent) {
        if event.id == 0 {
            crate::lighting::set_right_battery(event.state.0);
            CORE_MAILBOX.snapshot_changed();
        }
    }
}

/// Mirrors authoritative declarative state to the peripheral. Engine changes
/// transfer an atomic snapshot, while context-only changes use a guarded delta.
/// An acknowledgement or timeout makes reconnect/loss convergence explicit.
pub struct CentralReplication {
    generation: u8,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum TransferKind {
    FullSnapshot,
    ContextUpdate,
}

#[derive(Clone, Copy)]
struct PendingAck {
    generation: u8,
    revision: u32,
    kind: TransferKind,
}

pub const fn replication() -> CentralReplication {
    CentralReplication { generation: 0 }
}

impl CentralReplication {
    async fn export_replica() -> Option<StandardReplicaState<OVERLAY_CAPACITY, SCENE_CAPACITY>> {
        if CORE_MAILBOX
            .request(StandardCommand::ExportReplica(&REPLICA_SLOT))
            .await
            .is_err()
        {
            return None;
        }
        REPLICA_SLOT.take().ok()
    }

    async fn try_send_snapshot(&mut self) -> Option<PendingAck> {
        let snapshot = Self::export_replica().await?;
        self.generation = self.generation.wrapping_add(1);
        if crate::split_lighting::try_queue_snapshot(
            self.generation,
            &snapshot,
            crate::lighting::battery_statuses(),
        ) {
            Some(PendingAck {
                generation: self.generation,
                revision: snapshot.revision,
                kind: TransferKind::FullSnapshot,
            })
        } else {
            None
        }
    }

    async fn try_send_context_update(
        &mut self,
        last_acked_revision: Option<u32>,
    ) -> Option<PendingAck> {
        let snapshot = Self::export_replica().await?;
        self.generation = self.generation.wrapping_add(1);
        let kind = if Some(snapshot.revision) == last_acked_revision {
            let message = crate::split_lighting::Message::ContextUpdate {
                generation: self.generation,
                revision: snapshot.revision,
                context: snapshot.context,
                batteries: crate::lighting::battery_statuses(),
            }
            .encode();
            if rmk::split_app::SPLIT_APP_TX.try_send(message).is_err() {
                return None;
            }
            TransferKind::ContextUpdate
        } else {
            if !crate::split_lighting::try_queue_snapshot(
                self.generation,
                &snapshot,
                crate::lighting::battery_statuses(),
            ) {
                return None;
            }
            TransferKind::FullSnapshot
        };
        Some(PendingAck {
            generation: self.generation,
            revision: snapshot.revision,
            kind,
        })
    }
}

impl Runnable for CentralReplication {
    async fn run(&mut self) -> ! {
        let mut link = rmk::split_app::SPLIT_APP_LINK
            .receiver()
            .expect("lighting replication owns one split-link receiver");
        let mut lighting = LightingChangedEvent::subscriber();
        let mut layers = LayerChangeEvent::subscriber();
        let mut indicators = LedIndicatorEvent::subscriber();
        let mut battery = BatteryStatusEvent::subscriber();
        let mut peripheral_battery = PeripheralBatteryEvent::subscriber();
        let mut link_up = false;
        let mut full_dirty = true;
        let mut context_dirty = false;
        let mut awaiting_ack: Option<PendingAck> = None;
        let mut last_acked_revision = None;

        loop {
            if link_up && awaiting_ack.is_none() && (full_dirty || context_dirty) {
                let pending = if full_dirty {
                    self.try_send_snapshot().await
                } else {
                    self.try_send_context_update(last_acked_revision).await
                };
                match pending {
                    Some(pending) => {
                        awaiting_ack = Some(pending);
                        full_dirty = false;
                        context_dirty = false;
                    }
                    None => {
                        embassy_time::Timer::after_millis(50).await;
                        continue;
                    }
                }
            }

            let timeout = async {
                if awaiting_ack.is_some() {
                    embassy_time::Timer::after_millis(500).await;
                } else {
                    core::future::pending::<()>().await;
                }
            };
            match select4(
                link.changed(),
                lighting.next_event(),
                select(
                    select(layers.next_event(), indicators.next_event()),
                    select(battery.next_event(), peripheral_battery.next_event()),
                ),
                select(rmk::split_app::SPLIT_APP_RX.receive(), timeout),
            )
            .await
            {
                Either4::First(up) => {
                    link_up = up;
                    awaiting_ack = None;
                    last_acked_revision = None;
                    full_dirty = up;
                    context_dirty = false;
                }
                Either4::Second(_) => full_dirty = true,
                Either4::Third(Either::First(_)) => context_dirty = true,
                Either4::Third(Either::Second(Either::First(event))) => {
                    crate::lighting::set_left_battery(event.0);
                    context_dirty = true;
                }
                Either4::Third(Either::Second(Either::Second(event))) => {
                    if event.id == 0 {
                        crate::lighting::set_right_battery(event.state.0);
                    }
                    context_dirty = true;
                }
                Either4::Fourth(Either::First(data)) => {
                    if let Ok(crate::split_lighting::Message::Ack {
                        generation,
                        revision,
                    }) = crate::split_lighting::Message::decode(data)
                        && let Some(pending) = awaiting_ack
                        && pending.generation == generation
                        && pending.revision == revision
                    {
                        awaiting_ack = None;
                        if pending.kind == TransferKind::FullSnapshot {
                            last_acked_revision = Some(revision);
                        }
                    }
                }
                Either4::Fourth(Either::Second(())) => {
                    awaiting_ack = None;
                    full_dirty = link_up;
                }
            }
        }
    }
}

pub struct RemoteBootDispatcher;

impl Runnable for RemoteBootDispatcher {
    async fn run(&mut self) -> ! {
        loop {
            REMOTE_BOOT_REQUESTS.receive().await;
            let message = SplitAppData::new(&[BOOTLOADER_TAG]).expect("one-byte message");
            // Lighting deliberately drops frames when the one-slot split
            // queue is busy, but a bootloader command must not be dropped:
            // the host has already received an acknowledgement. Wait until
            // this control message owns the next available queue slot.
            rmk::split_app::SPLIT_APP_TX.send(message).await;
        }
    }
}
