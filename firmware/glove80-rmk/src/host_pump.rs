//! Host-protocol transport pumps (central only; see `host_proto.rs` for the
//! shared types and the request semantics).
//!
//! One pump per transport, both driven by [`TransportPump`] (registered as an
//! RMK processor from `central.rs`). Each pump handles one message at a time:
//! reassemble frames from RMK's raw pipes (`rmk::vendor_transport`),
//! decode, hand the request to the lighting task, await that request's
//! response, encode it, and frame it back out. Waiting for the response
//! before reading further frames enforces the protocol's "one message in
//! flight per direction per transport" rule by construction.
//!
//! This module is deliberately not compiled into the peripheral binary: the
//! host transports are central-only in Phase 2, and keeping the pumps out of
//! `peripheral.rs`'s module tree means no dead tasks or channels there.

use embassy_futures::join::join;
use embassy_time::Timer;
use glove80_host_protocol::frame::{FRAME_HEADER_LEN, Reassembler, frame_count, write_frame};
use glove80_host_protocol::{
    DecodeError, FrameError, MAX_MESSAGE_LEN, RESPONSE_FLAG, Status, decode_request,
    encode_response,
};
use rmk::event::{EventSubscriber, LayerChangeEvent, SubscribableEvent};
use rmk::processor::Processor;
use rmk::vendor_transport::{
    BLE_MAX_CHUNK_LEN, BleChunk, USB_REPORT_LEN, VENDOR_BLE_ATT_PAYLOAD, VENDOR_BLE_RX,
    VENDOR_BLE_TX, VENDOR_USB_RX, VENDOR_USB_TX,
};

use crate::host_proto::{BLE_RESPONSES, HOST_REQUESTS, HostRequest, Transport, USB_RESPONSES};

/// Both transport pumps, registered as one RMK processor from `central.rs`.
///
/// The `Processor` impl exists only because `#[register_processor(event)]`
/// drives registered tasks through `Processor::process_loop`; the overridden
/// loop below never creates the (unused) event subscriber, so no
/// `layer_change` subscriber slot is consumed.
pub struct TransportPump;

impl rmk::core_traits::Runnable for TransportPump {
    async fn run(&mut self) -> ! {
        self.process_loop().await
    }
}

impl Processor for TransportPump {
    type Event = LayerChangeEvent;

    fn subscriber() -> impl EventSubscriber<Event = LayerChangeEvent> {
        LayerChangeEvent::subscriber()
    }

    async fn process(&mut self, _event: LayerChangeEvent) {}

    async fn process_loop(&mut self) -> ! {
        join(usb_pump(), ble_pump()).await;
        unreachable!("transport pumps run forever")
    }
}

/// Map a request-decode failure to the status of its (empty) error response.
fn decode_error_status(e: DecodeError) -> Status {
    match e {
        DecodeError::UnknownOpcode(_) => Status::UnknownCommand,
        DecodeError::CapacityExceeded => Status::CapacityExceeded,
        _ => Status::Malformed,
    }
}

/// Handle one complete, reassembled request message: decode it, run it
/// through the lighting task, and encode the response into `out`. Returns
/// `Some((encoded_len, enter_bootloader))`, or `None` when no response can be
/// produced (message too short to even echo a request id).
async fn dispatch(
    transport: Transport,
    msg: &[u8],
    out: &mut [u8; MAX_MESSAGE_LEN],
) -> Option<(usize, bool)> {
    match decode_request(msg) {
        Ok((request_id, request)) => {
            HOST_REQUESTS
                .send(HostRequest {
                    transport,
                    request_id,
                    request,
                })
                .await;
            let resp = match transport {
                Transport::Usb => USB_RESPONSES.receive().await,
                Transport::Ble => BLE_RESPONSES.receive().await,
            };
            match encode_response(&resp.response, out) {
                Ok(len) => Some((len, resp.enter_bootloader)),
                Err(e) => {
                    defmt::error!(
                        "host-proto: response encode failed: {}",
                        defmt::Debug2Format(&e)
                    );
                    None
                }
            }
        }
        Err(e) => {
            // Every request gets exactly one response; errors echo the
            // request's opcode and id with an empty payload. Hand-rolled
            // because the codec's `Command` cannot represent unknown opcodes.
            if msg.len() < 2 {
                defmt::warn!("host-proto: message too short to answer");
                return None;
            }
            defmt::warn!("host-proto: bad request: {}", defmt::Debug2Format(&e));
            out[0] = msg[0] | RESPONSE_FLAG;
            out[1] = msg[1];
            out[2] = decode_error_status(e) as u8;
            out[3] = 0;
            out[4] = 0;
            Some((5, false))
        }
    }
}

/// Best-effort bootloader entry: give the transports a moment to flush the
/// OK response (clients tolerate never receiving it), then reboot via the
/// Adafruit bootloader GPREGRET magic.
async fn reboot_to_bootloader() -> ! {
    defmt::warn!("host-proto: entering bootloader");
    Timer::after_millis(300).await;
    rmk::boot::jump_to_bootloader();
    unreachable!("jump_to_bootloader resets the chip")
}

/// USB pump: 32-byte raw-HID reports in both directions, zero-padded.
async fn usb_pump() -> ! {
    let mut reasm: Reassembler<MAX_MESSAGE_LEN> = Reassembler::new();
    let mut out = [0u8; MAX_MESSAGE_LEN];
    loop {
        let report = VENDOR_USB_RX.receive().await;
        match reasm.push(&report) {
            Ok(Some(msg)) => {
                if let Some((len, reboot)) = dispatch(Transport::Usb, msg, &mut out).await {
                    if let Err(e) = send_usb_response(&out[..len]).await {
                        defmt::error!(
                            "host-proto: USB framing failed: {}",
                            defmt::Debug2Format(&e)
                        );
                    } else if reboot {
                        reboot_to_bootloader().await;
                    }
                }
            }
            Ok(None) => {}
            Err(e) => defmt::warn!("host-proto: USB frame error: {}", defmt::Debug2Format(&e)),
        }
    }
}

/// Chunk one encoded response message into padded 32-byte IN reports.
async fn send_usb_response(msg: &[u8]) -> Result<(), FrameError> {
    let frames = frame_count(msg.len(), USB_REPORT_LEN)?;
    for index in 0..frames {
        let mut report = [0u8; USB_REPORT_LEN];
        write_frame(msg, USB_REPORT_LEN, index, &mut report)?;
        VENDOR_USB_TX.send(report).await;
    }
    Ok(())
}

/// BLE pump: variable-length ATT chunks; responses sized to the negotiated
/// ATT payload RMK's vendor GATT layer reports.
async fn ble_pump() -> ! {
    let mut reasm: Reassembler<MAX_MESSAGE_LEN> = Reassembler::new();
    let mut out = [0u8; MAX_MESSAGE_LEN];
    loop {
        let chunk = VENDOR_BLE_RX.receive().await;
        match reasm.push(&chunk.data[..chunk.len as usize]) {
            Ok(Some(msg)) => {
                if let Some((len, reboot)) = dispatch(Transport::Ble, msg, &mut out).await {
                    if let Err(e) = send_ble_response(&out[..len]).await {
                        defmt::error!(
                            "host-proto: BLE framing failed: {}",
                            defmt::Debug2Format(&e)
                        );
                    } else if reboot {
                        reboot_to_bootloader().await;
                    }
                }
            }
            Ok(None) => {}
            Err(e) => defmt::warn!("host-proto: BLE frame error: {}", defmt::Debug2Format(&e)),
        }
    }
}

/// Chunk one encoded response into notify-sized chunks: the frame payload is
/// `min(negotiated ATT payload - FRAME_HEADER_LEN, 255)` per PROTOCOL.md.
async fn send_ble_response(msg: &[u8]) -> Result<(), FrameError> {
    let att_payload = VENDOR_BLE_ATT_PAYLOAD.load(core::sync::atomic::Ordering::Relaxed) as usize;
    let chunk_len = att_payload.clamp(FRAME_HEADER_LEN + 1, BLE_MAX_CHUNK_LEN);
    let frames = frame_count(msg.len(), chunk_len)?;
    for index in 0..frames {
        let mut chunk = BleChunk::empty();
        let used = write_frame(msg, chunk_len, index, &mut chunk.data)?;
        chunk.len = used as u16;
        VENDOR_BLE_TX.send(chunk).await;
    }
    Ok(())
}
