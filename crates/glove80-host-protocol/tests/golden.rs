//! Golden vector suite for protocol v1.0.
//!
//! The vector file `crates/glove80-host-protocol/vectors/host-protocol-v1.json` is *generated*
//! from the message constructions in this test (run with
//! `GLOVE80_WRITE_VECTORS=1 cargo test --test golden` to regenerate) and
//! consumed by both this suite and the TypeScript suite
//! (`ui/src/lib/host-protocol.test.ts`), so the two codecs cannot drift.
//!
//! These vectors are **frozen at v1.0**: version numbers below are literal
//! `1` / `0` (not the crate constants) so that later minor bumps can never
//! change a v1.0 byte. v1.1 vectors live in `golden_v11.rs` /
//! `host-protocol-v1.1.json`.

mod common;

use common::{heapless_bytes, message_vectors, Message};
use glove80_host_protocol::frame::write_frame;
use glove80_host_protocol::{
    BootTarget, Capabilities, CellState, CellWrite, Command, Effect, Request, Response,
    ResponsePayload, Status, BOOTLOADER_MAGIC, MAX_MESSAGE_LEN,
};
use serde_json::{json, Value};

fn messages() -> Vec<(&'static str, Message)> {
    use Message::{Req, Resp};

    let blink = Effect::blink(255, 0, 64, 1000, 250, 50);
    let green = Effect::solid(0, 255, 0);
    let breathe = Effect::breathe(16, 32, 48, 3000, 0);

    let two_cells = heapless::Vec::from_slice(&[
        CellWrite { key: 12, effect: blink },
        CellWrite { key: 41, effect: green },
    ])
    .unwrap();
    let one_cell = heapless::Vec::from_slice(&[CellWrite { key: 0, effect: breathe }]).unwrap();

    let caps = Capabilities {
        protocol_major: 1,
        protocol_minor: 0,
        led_count_left: 40,
        led_count_right: 40,
        layer_capacity: 8,
        max_cells_per_op: 80,
        effect_mask: 0b0000_0111,
        overlay_cell_capacity: 80,
        max_message_len: MAX_MESSAGE_LEN as u16,
        feature_bits: 0x3F,
        // Not on the wire: the persistent-config and keymap feature bits are
        // clear.
        max_config_blob_len: 0,
        keymap_rows: 0,
        keymap_cols: 0,
        max_keymap_entries_per_op: 0,
    };

    vec![
        (
            "get_capabilities_request",
            Req(1, Request::GetCapabilities { client_major: 1, client_minor: 0 }),
        ),
        (
            "get_capabilities_response",
            Resp(Response {
                request_id: 1,
                command: Command::GetCapabilities,
                status: Status::Ok,
                payload: ResponsePayload::Capabilities(caps),
            }),
        ),
        (
            "ping_request",
            Req(2, Request::Ping { data: heapless_bytes(&[0xDE, 0xAD, 0xBE, 0xEF]) }),
        ),
        (
            "ping_response",
            Resp(Response {
                request_id: 2,
                command: Command::Ping,
                status: Status::Ok,
                payload: ResponsePayload::Echo { data: heapless_bytes(&[0xDE, 0xAD, 0xBE, 0xEF]) },
            }),
        ),
        ("ping_empty_request", Req(255, Request::Ping { data: heapless::Vec::new() })),
        (
            "set_cells_request",
            Req(3, Request::SetCells { ttl_ms: 5000, cells: two_cells.clone() }),
        ),
        (
            "set_cells_response_ok",
            Resp(Response {
                request_id: 3,
                command: Command::SetCells,
                status: Status::Ok,
                payload: ResponsePayload::OverlayAck { pending_keys: heapless::Vec::new() },
            }),
        ),
        (
            "set_cells_response_partial_apply",
            Resp(Response {
                request_id: 4,
                command: Command::SetCells,
                status: Status::PartialApply,
                payload: ResponsePayload::OverlayAck { pending_keys: heapless_bytes(&[41, 42]) },
            }),
        ),
        ("unset_cells_request", Req(5, Request::UnsetCells { keys: heapless_bytes(&[12, 41]) })),
        (
            "unset_cells_response",
            Resp(Response {
                request_id: 5,
                command: Command::UnsetCells,
                status: Status::Ok,
                payload: ResponsePayload::OverlayAck { pending_keys: heapless::Vec::new() },
            }),
        ),
        ("clear_overlay_request", Req(6, Request::ClearOverlay)),
        (
            "clear_overlay_response",
            Resp(Response {
                request_id: 6,
                command: Command::ClearOverlay,
                status: Status::Ok,
                payload: ResponsePayload::OverlayAck { pending_keys: heapless::Vec::new() },
            }),
        ),
        ("read_overlay_request", Req(7, Request::ReadOverlay)),
        (
            "read_overlay_response",
            Resp(Response {
                request_id: 7,
                command: Command::ReadOverlay,
                status: Status::Ok,
                payload: ResponsePayload::OverlayState {
                    cells: heapless::Vec::from_slice(&[
                        CellState { key: 12, effect: blink, remaining_ttl_ms: 4200 },
                        CellState { key: 41, effect: green, remaining_ttl_ms: 0 },
                    ])
                    .unwrap(),
                },
            }),
        ),
        (
            "read_overlay_response_empty",
            Resp(Response {
                request_id: 8,
                command: Command::ReadOverlay,
                status: Status::Ok,
                payload: ResponsePayload::OverlayState { cells: heapless::Vec::new() },
            }),
        ),
        (
            "replace_overlay_request",
            Req(9, Request::ReplaceOverlay { ttl_ms: 0, cells: one_cell }),
        ),
        (
            "replace_overlay_response",
            Resp(Response {
                request_id: 9,
                command: Command::ReplaceOverlay,
                status: Status::Ok,
                payload: ResponsePayload::OverlayAck { pending_keys: heapless::Vec::new() },
            }),
        ),
        ("get_brightness_request", Req(10, Request::GetBrightness)),
        (
            "get_brightness_response",
            Resp(Response {
                request_id: 10,
                command: Command::GetBrightness,
                status: Status::Ok,
                payload: ResponsePayload::Brightness { level: 128 },
            }),
        ),
        ("set_brightness_request", Req(11, Request::SetBrightness { level: 192 })),
        (
            "set_brightness_response",
            Resp(Response {
                request_id: 11,
                command: Command::SetBrightness,
                status: Status::Ok,
                payload: ResponsePayload::Brightness { level: 192 },
            }),
        ),
        ("get_toggle_request", Req(12, Request::GetToggle { id: 2 })),
        (
            "get_toggle_response",
            Resp(Response {
                request_id: 12,
                command: Command::GetToggle,
                status: Status::Ok,
                payload: ResponsePayload::Toggle { id: 2, state: true },
            }),
        ),
        ("set_toggle_request", Req(13, Request::SetToggle { id: 2, state: false })),
        (
            "set_toggle_response",
            Resp(Response {
                request_id: 13,
                command: Command::SetToggle,
                status: Status::Ok,
                payload: ResponsePayload::Toggle { id: 2, state: false },
            }),
        ),
        (
            "set_toggle_response_unknown",
            Resp(Response {
                request_id: 14,
                command: Command::SetToggle,
                status: Status::UnknownToggle,
                payload: ResponsePayload::Empty,
            }),
        ),
        (
            "enter_bootloader_request",
            Req(15, Request::EnterBootloader {
                magic: BOOTLOADER_MAGIC,
                target: BootTarget::Peripheral,
            }),
        ),
        (
            "enter_bootloader_response_ok",
            Resp(Response {
                request_id: 15,
                command: Command::EnterBootloader,
                status: Status::Ok,
                payload: ResponsePayload::Empty,
            }),
        ),
        (
            "enter_bootloader_response_bad_magic",
            Resp(Response {
                request_id: 16,
                command: Command::EnterBootloader,
                status: Status::BadMagic,
                payload: ResponsePayload::Empty,
            }),
        ),
    ]
}

fn frame_vectors() -> Vec<Value> {
    // (name, message-name to frame, chunk size, pad to chunk size?)
    let plans = [
        ("set_cells_frames_hid32_padded", "set_cells_request", 32usize, true),
        ("set_cells_frames_ble20", "set_cells_request", 20, false),
        ("ping_frames_hid32_padded", "ping_request", 32, true),
    ];
    let all = messages();
    plans
        .iter()
        .map(|(name, source, chunk, pad)| {
            let (_, m) = all.iter().find(|(n, _)| n == source).unwrap();
            let message = common::encode_message(m);
            let count = glove80_host_protocol::frame::frame_count(message.len(), *chunk).unwrap();
            let frames: Vec<String> = (0..count)
                .map(|i| {
                    let mut out = vec![0u8; *chunk];
                    let used = write_frame(&message, *chunk, i, &mut out).unwrap();
                    if !*pad {
                        out.truncate(used);
                    }
                    common::hex(&out)
                })
                .collect();
            json!({
                "name": name,
                "sourceMessage": source,
                "chunkSize": chunk,
                "padded": pad,
                "messageHex": common::hex(&message),
                "framesHex": frames,
            })
        })
        .collect()
}

fn golden_doc() -> Value {
    json!({
        // Frozen v1.0 stamp; see the module comment.
        "protocol": { "major": 1, "minor": 0 },
        "generatedBy": "crates/glove80-host-protocol tests/golden.rs (GLOVE80_WRITE_VECTORS=1 cargo test --test golden)",
        "messages": message_vectors(&messages()),
        "frames": frame_vectors(),
    })
}

const FILE_NAME: &str = "host-protocol-v1.json";

#[test]
fn golden_file_matches_generator() {
    let doc = golden_doc();
    if common::maybe_write(FILE_NAME, &doc) {
        return;
    }
    assert_eq!(
        doc,
        common::load_file(FILE_NAME),
        "vector file is stale; regenerate with GLOVE80_WRITE_VECTORS=1 cargo test --test golden"
    );
}

#[test]
fn golden_messages_decode() {
    common::assert_messages_decode(&common::load_file(FILE_NAME), &messages());
}

#[test]
fn golden_frames_reassemble() {
    let file = common::load_file(FILE_NAME);
    for entry in file["frames"].as_array().unwrap() {
        let name = entry["name"].as_str().unwrap();
        let message = common::unhex(entry["messageHex"].as_str().unwrap());
        let mut reassembler: glove80_host_protocol::frame::Reassembler<MAX_MESSAGE_LEN> =
            glove80_host_protocol::frame::Reassembler::new();
        let frames = entry["framesHex"].as_array().unwrap();
        for (i, f) in frames.iter().enumerate() {
            let chunk = common::unhex(f.as_str().unwrap());
            let out = reassembler.push(&chunk).unwrap_or_else(|e| panic!("push {name}: {e:?}"));
            if i == frames.len() - 1 {
                assert_eq!(out.expect("final frame yields message"), &message[..], "{name}");
            } else {
                assert!(out.is_none(), "{name}: message completed early");
            }
        }
    }
}
