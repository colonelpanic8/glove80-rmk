//! Central ownership of the board-wide RMK lighting engine and Rynk state.
//!
//! The central is the configuration/control authority. Both halves render
//! board-wide declarative state locally; this module mirrors atomic semantic
//! snapshots rather than streaming sampled right-half RGB frames.

use embassy_futures::select::{Either, Either4, select, select4};
use embassy_nrf::Peri;
use embassy_nrf::gpio::Pin;
use embassy_nrf::peripherals::{PWM0, SPI3};
use embassy_time::{Duration, Instant, Timer};
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
    LightingExtendedConditionalSceneCell, LightingLayerPolicy, LightingSceneCell,
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
    persisted_runtime_conditional_scenes: &[LightingExtendedConditionalSceneCell],
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
    let provider = KeymapLightingState::new(keymap).expect("board layer count fits lighting state");
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
    /// Absolute instant this transfer stops waiting and is resent. A deadline
    /// rather than a per-iteration timer: rearming a fresh timeout every time
    /// the task woke for something else meant a stream of layer changes could
    /// postpone retransmission indefinitely.
    deadline: Instant,
}

/// How long a transfer waits for the peripheral's acknowledgement.
const ACK_TIMEOUT: Duration = Duration::from_millis(500);
/// How long the central waits before re-offering a transfer the split queue
/// had no room for.
const SEND_BACKOFF: Duration = Duration::from_millis(50);

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
                deadline: Instant::now() + ACK_TIMEOUT,
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
            deadline: Instant::now() + ACK_TIMEOUT,
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
        // Backoff deadline after the split queue refused a transfer. A
        // deadline the select waits on, never an inline sleep: sleeping here
        // meant a full queue made this task deaf to acks, events, and above
        // all the link going down -- it would spin re-offering snapshots to a
        // dead queue forever, which is exactly how the peripheral's replicated
        // context froze for good.
        let mut retry_at: Option<Instant> = None;

        loop {
            if retry_at.is_some_and(|at| at <= Instant::now()) {
                retry_at = None;
            }
            if link_up
                && awaiting_ack.is_none()
                && (full_dirty || context_dirty)
                && retry_at.is_none()
            {
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
                    None => retry_at = Some(Instant::now() + SEND_BACKOFF),
                }
            }

            // The one wake-up deadline: the outstanding ack's, or the send
            // backoff's, whichever lands first. `Timer::at` keeps them
            // absolute, so a wake for any other arm cannot postpone them.
            let deadline = match (
                awaiting_ack.as_ref().map(|pending| pending.deadline),
                retry_at,
            ) {
                (Some(ack), Some(retry)) => Some(ack.min(retry)),
                (deadline, None) | (None, deadline) => deadline,
            };
            let timeout = async {
                match deadline {
                    Some(at) => Timer::at(at).await,
                    None => core::future::pending::<()>().await,
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
                    retry_at = None;
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
                    // One timer serves two deadlines; only the one that has
                    // actually elapsed may act. An elapsed send backoff is
                    // cleared at the top of the loop.
                    if awaiting_ack
                        .as_ref()
                        .is_some_and(|pending| pending.deadline <= Instant::now())
                    {
                        awaiting_ack = None;
                        full_dirty = link_up;
                    }
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
