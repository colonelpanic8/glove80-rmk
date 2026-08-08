//! Central ownership of the board-wide RMK lighting engine and Rynk state.
//!
//! The central is the configuration/control authority. Both halves render
//! board-wide declarative state locally; this module mirrors atomic semantic
//! snapshots rather than streaming sampled right-half RGB frames.

use core::cell::Cell;

use embassy_futures::select::{Either, Either4, select, select4};
use embassy_nrf::Peri;
use embassy_nrf::gpio::Pin;
use embassy_nrf::peripherals::SPI3;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_time::{Duration, Instant, Timer};
use rmk::core_traits::Runnable;
use rmk::event::{
    BatteryStatusEvent, EventSubscriber, LayerChangeEvent, LedIndicatorEvent, LightingChangedEvent,
    PeripheralBatteryEvent, SubscribableEvent,
};
use rmk::host::{
    LightingReplicationStatus, PeripheralReplicaStatus, RemoteFrame, RemoteFramePort,
    ReplicationHealth, ReplicationMachineState, RynkLightingController, RynkLightingDescriptor,
    RynkLightingMailbox, StandardRynkLightingAdapter, install_lighting_runtime_conditional_scenes,
    install_lighting_scenes,
};
use rmk::keymap::KeyMap;
use rmk::lighting::{
    FramePage, KeymapLightingState, LightingNodeId, LightingProcessor, LightingService,
    LogicalFrame, Rgb8, StandardCommand, StandardReplicaState,
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
static REMOTE_FRAMES: RemoteFramePort = RemoteFramePort::new();
static STATUS_REFRESH_REQUESTS: embassy_sync::channel::Channel<rmk::RawMutex, (), 1> =
    embassy_sync::channel::Channel::new();
static FRAME_RESPONSES: embassy_sync::channel::Channel<
    rmk::RawMutex,
    crate::split_lighting::Message,
    1,
> = embassy_sync::channel::Channel::new();

#[derive(Clone, Copy)]
struct CachedPeripheral {
    applied_revision: Option<u32>,
    engine_revision: u32,
    layers: rmk::lighting::LayerState,
    powered: bool,
    wake_active: bool,
    effective_output_enabled: bool,
    received_at: Instant,
    digests: Option<rmk::host::ReplicaDigests>,
}

#[derive(Clone, Copy)]
struct ObservabilityCache {
    machine: ReplicationMachineState,
    peripheral: Option<CachedPeripheral>,
    last_attested_at: Option<Instant>,
    last_refresh_requested_at: Option<Instant>,
}

static OBSERVABILITY_CACHE: BlockingMutex<rmk::RawMutex, Cell<ObservabilityCache>> =
    BlockingMutex::new(Cell::new(ObservabilityCache {
        machine: ReplicationMachineState {
            last_acked_revision: None,
            awaiting_ack: false,
            generation: 0,
            link_up: false,
            durable_dirty: true,
            context_dirty: false,
            health: ReplicationHealth::Stale,
            expected_digests: None,
            last_attested_age_ms: None,
            mismatch_count: 0,
        },
        peripheral: None,
        last_attested_at: None,
        last_refresh_requested_at: None,
    }));

pub struct BoardReplicationStatus;

static REPLICATION_STATUS: BoardReplicationStatus = BoardReplicationStatus;

impl LightingReplicationStatus for BoardReplicationStatus {
    fn request_refresh(&self) {
        const MIN_REFRESH_INTERVAL: Duration = Duration::from_millis(100);
        let now = Instant::now();
        let eligible = OBSERVABILITY_CACHE.lock(|slot| {
            let mut cache = slot.get();
            if cache
                .last_refresh_requested_at
                .is_some_and(|last| now < last + MIN_REFRESH_INTERVAL)
            {
                return false;
            }
            cache.last_refresh_requested_at = Some(now);
            slot.set(cache);
            true
        });
        if eligible {
            let _ = STATUS_REFRESH_REQUESTS.try_send(());
        }
    }

    fn central(&self) -> ReplicationMachineState {
        let now = Instant::now();
        OBSERVABILITY_CACHE.lock(|cache| {
            let cache = cache.get();
            let mut machine = cache.machine;
            machine.last_attested_age_ms = cache.last_attested_at.map(|at| {
                now.saturating_duration_since(at)
                    .as_millis()
                    .min(u32::MAX as u64) as u32
            });
            machine
        })
    }

    fn peripheral(&self, node: LightingNodeId) -> Option<PeripheralReplicaStatus> {
        if node != LightingNodeId(1) {
            return None;
        }
        let now = Instant::now();
        OBSERVABILITY_CACHE.lock(|cache| {
            let peripheral = cache.get().peripheral?;
            Some(PeripheralReplicaStatus {
                applied_revision: peripheral.applied_revision,
                engine_revision: peripheral.engine_revision,
                layers: peripheral.layers,
                powered: peripheral.powered,
                wake_active: peripheral.wake_active,
                effective_output_enabled: peripheral.effective_output_enabled,
                age_ms: now
                    .saturating_duration_since(peripheral.received_at)
                    .as_millis()
                    .min(u32::MAX as u64) as u32,
                digests: peripheral.digests,
            })
        })
    }
}

fn set_machine(machine: ReplicationMachineState, last_attested_at: Option<Instant>) {
    OBSERVABILITY_CACHE.lock(|slot| {
        let mut cache = slot.get();
        cache.machine = machine;
        cache.last_attested_at = last_attested_at;
        slot.set(cache);
    });
}

fn record_attestation(digests: rmk::host::ReplicaDigests) {
    OBSERVABILITY_CACHE.lock(|slot| {
        let mut cache = slot.get();
        match cache.peripheral.as_mut() {
            Some(peripheral) => {
                peripheral.applied_revision = Some(digests.revision);
                peripheral.digests = Some(digests);
            }
            None => {
                cache.peripheral = Some(CachedPeripheral {
                    applied_revision: Some(digests.revision),
                    engine_revision: digests.revision,
                    layers: rmk::lighting::LayerState::new(0, 0, 0),
                    powered: false,
                    wake_active: false,
                    effective_output_enabled: false,
                    received_at: Instant::now(),
                    digests: Some(digests),
                });
            }
        }
        slot.set(cache);
    });
}

fn record_status(
    applied_revision: Option<u32>,
    engine_revision: u32,
    layers: rmk::lighting::LayerState,
    powered: bool,
    wake_active: bool,
    effective_output_enabled: bool,
) {
    OBSERVABILITY_CACHE.lock(|slot| {
        let mut cache = slot.get();
        let digests = cache.peripheral.and_then(|peripheral| peripheral.digests);
        cache.peripheral = Some(CachedPeripheral {
            applied_revision,
            engine_revision,
            layers,
            powered,
            wake_active,
            effective_output_enabled,
            received_at: Instant::now(),
            digests,
        });
        slot.set(cache);
    });
}

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
    let output = HalfOutput::left(LightingHardware::new(spi, data_pin, chain_power_pin));
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
    .with_remote_frames(&REMOTE_FRAMES)
    .with_replication_status(&REPLICATION_STATUS)
}

pub struct RemoteFrameBridge {
    next_request_id: u8,
}

pub const fn remote_frame_bridge() -> RemoteFrameBridge {
    RemoteFrameBridge { next_request_id: 0 }
}

impl RemoteFrameBridge {
    async fn chunk(
        &mut self,
        offset: u16,
        deadline: Instant,
    ) -> Option<crate::split_lighting::Message> {
        const ATTEMPT_TIMEOUT: Duration = Duration::from_millis(60);
        for _ in 0..2 {
            if Instant::now() >= deadline {
                return None;
            }
            self.next_request_id = self.next_request_id.wrapping_add(1);
            let request_id = self.next_request_id;
            let request =
                crate::split_lighting::Message::FrameChunkRequest { request_id, offset }.encode();
            if rmk::split_app::SPLIT_APP_TX.try_send(request).is_err() {
                Timer::after_millis(5).await;
                continue;
            }
            let attempt_deadline = (Instant::now() + ATTEMPT_TIMEOUT).min(deadline);
            loop {
                match select(FRAME_RESPONSES.receive(), Timer::at(attempt_deadline)).await {
                    Either::First(
                        message @ crate::split_lighting::Message::FrameChunk {
                            request_id: response_id,
                            ..
                        },
                    ) if response_id == request_id => return Some(message),
                    Either::First(_) => {}
                    Either::Second(()) => break,
                }
            }
        }
        None
    }

    async fn read_page(&mut self, offset: u16, deadline: Instant) -> Option<FramePage> {
        for _ in 0..2 {
            let mut assembly = crate::split_lighting::FramePageAssembly::new(offset);
            loop {
                let start = offset.saturating_add(assembly.len() as u16);
                let Some(chunk) = self.chunk(start, deadline).await else {
                    return None;
                };
                match assembly.accept(chunk) {
                    crate::split_lighting::FrameChunkDecision::Accepted => {}
                    crate::split_lighting::FrameChunkDecision::Complete => {
                        return Some(FramePage {
                            revision: assembly.revision(),
                            total: assembly.total(),
                            start: assembly.start(),
                            len: assembly.len() as u8,
                            cells: *assembly.cells(),
                        });
                    }
                    crate::split_lighting::FrameChunkDecision::Restart => break,
                    crate::split_lighting::FrameChunkDecision::Invalid => return None,
                }
            }
            if Instant::now() >= deadline {
                return None;
            }
        }
        None
    }
}

impl Runnable for RemoteFrameBridge {
    async fn run(&mut self) -> ! {
        const PAGE_DEADLINE: Duration = Duration::from_millis(900);
        loop {
            let request = REMOTE_FRAMES.receive().await;
            let started = Instant::now();
            let frame = if request.node == LightingNodeId(1)
                && rmk::split_app::SPLIT_APP_LINK.try_get() == Some(true)
            {
                self.read_page(request.offset, started + PAGE_DEADLINE)
                    .await
                    .map(|page| RemoteFrame {
                        page,
                        age_ms: Instant::now()
                            .saturating_duration_since(started)
                            .as_millis()
                            .min(u32::MAX as u64) as u32,
                    })
            } else {
                None
            };
            REMOTE_FRAMES.reply(request.id, frame);
        }
    }
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
    digests: Option<rmk::host::ReplicaDigests>,
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
        let digests = crate::split_lighting::replica_digests(&snapshot);
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
                digests: Some(digests),
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
                context: snapshot.context.into(),
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
            digests: (kind == TransferKind::FullSnapshot)
                .then(|| crate::split_lighting::replica_digests(&snapshot)),
        })
    }
}

impl Runnable for CentralReplication {
    async fn run(&mut self) -> ! {
        let mut link = rmk::split_app::SPLIT_APP_LINK
            .receiver()
            .expect("lighting replication owns one split-link receiver");
        let mut lighting = LightingChangedEvent::subscriber();
        // Preserve the established central task/future layout without treating
        // layer activity as replicated context. RMK's native split driver owns
        // delivery; this arm intentionally performs no synchronization work.
        let mut layers = LayerChangeEvent::subscriber();
        let mut indicators = LedIndicatorEvent::subscriber();
        let mut battery = BatteryStatusEvent::subscriber();
        let mut peripheral_battery = PeripheralBatteryEvent::subscriber();
        let mut link_up = false;
        let mut full_dirty = true;
        let mut context_dirty = false;
        let mut awaiting_ack: Option<PendingAck> = None;
        let mut last_acked_revision = None;
        let mut expected_digests = None;
        let mut health = ReplicationHealth::Resynchronizing;
        let mut last_attested_at = None;
        let mut recovery = crate::split_lighting::AttestationRecovery::new();
        let mut status_request_id = 0u8;
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
                && health != ReplicationHealth::Halted
            {
                let pending = if full_dirty {
                    self.try_send_snapshot().await
                } else {
                    self.try_send_context_update(last_acked_revision).await
                };
                match pending {
                    Some(pending) => {
                        if let Some(digests) = pending.digests {
                            expected_digests = Some(digests);
                        }
                        awaiting_ack = Some(pending);
                        full_dirty = false;
                        context_dirty = false;
                        health = ReplicationHealth::Resynchronizing;
                    }
                    None => retry_at = Some(Instant::now() + SEND_BACKOFF),
                }
            }

            set_machine(
                ReplicationMachineState {
                    last_acked_revision,
                    awaiting_ack: awaiting_ack.is_some(),
                    generation: self.generation,
                    link_up,
                    durable_dirty: full_dirty,
                    context_dirty,
                    health,
                    expected_digests,
                    last_attested_age_ms: None,
                    mismatch_count: recovery.mismatch_count(),
                },
                last_attested_at,
            );

            // The one wake-up deadline: the outstanding ack's, or the send
            // backoff's, whichever lands first. `Timer::at` keeps them
            // absolute, so a wake for any other arm cannot postpone them.
            let deadline = [
                awaiting_ack.as_ref().map(|pending| pending.deadline),
                retry_at,
            ]
            .into_iter()
            .flatten()
            .min();
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
                select(
                    select(
                        rmk::split_app::SPLIT_APP_RX.receive(),
                        STATUS_REFRESH_REQUESTS.receive(),
                    ),
                    timeout,
                ),
            )
            .await
            {
                Either4::First(up) => {
                    link_up = up;
                    awaiting_ack = None;
                    // Replicate directly on reconnect. Probing first observes the
                    // peripheral's necessarily stale pre-sync digest and feeds a
                    // full-resync loop that can starve the hardware watchdog.
                    last_acked_revision = None;
                    full_dirty = up;
                    context_dirty = false;
                    retry_at = None;
                    recovery.reset();
                    health = if up {
                        ReplicationHealth::Resynchronizing
                    } else {
                        ReplicationHealth::Stale
                    };
                }
                Either4::Second(_) => {
                    full_dirty = true;
                    recovery.reset();
                    if link_up {
                        health = ReplicationHealth::Resynchronizing;
                    }
                }
                Either4::Third(Either::First(Either::First(_))) => {}
                Either4::Third(Either::First(Either::Second(_))) => context_dirty = true,
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
                Either4::Fourth(Either::First(Either::First(data))) => {
                    match crate::split_lighting::Message::decode(data) {
                        Ok(crate::split_lighting::Message::Ack {
                            generation,
                            revision,
                        }) if awaiting_ack.is_some_and(|pending| {
                            pending.generation == generation && pending.revision == revision
                        }) =>
                        {
                            let pending = awaiting_ack.take().expect("guarded above");
                            if pending.kind == TransferKind::FullSnapshot {
                                last_acked_revision = Some(revision);
                                recovery.snapshot_acked(revision);
                            }
                            health = ReplicationHealth::Healthy;
                        }
                        Ok(crate::split_lighting::Message::Attestation { digests, .. }) => {
                            // Attestation is diagnostic only. Normal mutations and
                            // reconnects already drive bounded snapshot replication;
                            // a mismatch must not recursively dirty another snapshot.
                            record_attestation(digests);
                            last_attested_at = Some(Instant::now());
                            health = match recovery.observe(expected_digests, digests) {
                                crate::split_lighting::AttestationDecision::Matching => {
                                    ReplicationHealth::Healthy
                                }
                                crate::split_lighting::AttestationDecision::Recover => {
                                    ReplicationHealth::Diverged
                                }
                                crate::split_lighting::AttestationDecision::Stale => {
                                    ReplicationHealth::Stale
                                }
                                crate::split_lighting::AttestationDecision::Halt => {
                                    ReplicationHealth::Diverged
                                }
                            };
                        }
                        Ok(crate::split_lighting::Message::StatusReport {
                            applied_revision,
                            engine_revision,
                            layers,
                            powered,
                            wake_active,
                            effective_output_enabled,
                            ..
                        }) => record_status(
                            applied_revision,
                            engine_revision,
                            layers,
                            powered,
                            wake_active,
                            effective_output_enabled,
                        ),
                        Ok(message @ crate::split_lighting::Message::FrameChunk { .. }) => {
                            let _ = FRAME_RESPONSES.try_send(message);
                        }
                        _ => {}
                    }
                }
                Either4::Fourth(Either::First(Either::Second(()))) => {
                    if link_up {
                        status_request_id = status_request_id.wrapping_add(1);
                        let _ = rmk::split_app::SPLIT_APP_TX.try_send(
                            crate::split_lighting::Message::StatusRequest {
                                request_id: status_request_id,
                            }
                            .encode(),
                        );
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
                        health = ReplicationHealth::Resynchronizing;
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
