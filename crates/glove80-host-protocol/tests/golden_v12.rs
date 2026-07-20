//! Golden vector suite for the protocol v1.2 additions (keymap editing).
//!
//! Generates/checks `crates/glove80-host-protocol/vectors/host-protocol-v1.2.json`, consumed by
//! this suite and the TypeScript suite. The v1.0/v1.1/v1.2 vectors are all
//! frozen now that v1.3 exists (`golden_v13.rs` is the current-version
//! suite): the 1/2 version literals below are deliberate and never change.

mod common;

use common::{message_vectors, Message};
use glove80_host_protocol::{
    Capabilities, Command, KeymapEntry, Request, Response, ResponsePayload, Status,
    MAX_CONFIG_BLOB_LEN, MAX_MESSAGE_LEN,
};
use serde_json::{json, Value};

const FILE_NAME: &str = "host-protocol-v1.2.json";

/// The capability shape the Glove80 firmware advertises at v1.2: all eight
/// feature bits, both capability extensions (config + keymap), the 6x14 grid.
fn glove80_caps() -> Capabilities {
    Capabilities {
        protocol_major: 1,
        protocol_minor: 2,
        led_count_left: 40,
        led_count_right: 40,
        layer_capacity: 8,
        max_cells_per_op: 80,
        effect_mask: 0b0000_0111,
        overlay_cell_capacity: 80,
        max_message_len: MAX_MESSAGE_LEN as u16,
        feature_bits: 0xFF,
        max_config_blob_len: MAX_CONFIG_BLOB_LEN as u32,
        keymap_rows: 6,
        keymap_cols: 14,
        max_keymap_entries_per_op: 84,
    }
}

fn messages() -> Vec<(&'static str, Message)> {
    use Message::{Req, Resp};

    let empty_err = |id: u8, command: Command, status: Status| {
        Resp(Response { request_id: id, command, status, payload: ResponsePayload::Empty })
    };

    // A read batch mixing plain HID keycodes, KC_NO (holes read back as
    // 0x0000), KC_TRNS, a layer-tap and a Vial user keycode.
    let read_keycodes: heapless::Vec<u16, 128> =
        heapless::Vec::from_slice(&[0x0004, 0x0000, 0x0001, 0x4229, 0x7E00]).unwrap();
    let write_entries: heapless::Vec<KeymapEntry, 128> = heapless::Vec::from_slice(&[
        KeymapEntry { layer: 0, key: 0, keycode: 0x0004 },  // A at (0,0)
        KeymapEntry { layer: 1, key: 42, keycode: 0x5220 }, // MO(0) at (3,0)
        KeymapEntry { layer: 7, key: 83, keycode: 0x0000 }, // KC_NO at (5,13)
    ])
    .unwrap();
    let write_readback: heapless::Vec<u16, 128> =
        heapless::Vec::from_slice(&[0x0004, 0x5220, 0x0000]).unwrap();

    vec![
        (
            "get_capabilities_response_keymap",
            Resp(Response {
                request_id: 50,
                command: Command::GetCapabilities,
                status: Status::Ok,
                payload: ResponsePayload::Capabilities(glove80_caps()),
            }),
        ),
        (
            "keymap_read_request",
            Req(51, Request::KeymapRead { layer: 2, start_key: 14, max_count: 5 }),
        ),
        (
            "keymap_read_response",
            Resp(Response {
                request_id: 51,
                command: Command::KeymapRead,
                status: Status::Ok,
                payload: ResponsePayload::KeymapActions {
                    layer: 2,
                    start_key: 14,
                    keycodes: read_keycodes,
                },
            }),
        ),
        (
            "keymap_read_request_full_layer",
            Req(52, Request::KeymapRead { layer: 0, start_key: 0, max_count: 84 }),
        ),
        (
            "keymap_read_response_out_of_range",
            empty_err(53, Command::KeymapRead, Status::OutOfRange),
        ),
        ("keymap_write_request", Req(54, Request::KeymapWrite { entries: write_entries })),
        (
            "keymap_write_response",
            Resp(Response {
                request_id: 54,
                command: Command::KeymapWrite,
                status: Status::Ok,
                payload: ResponsePayload::KeymapWritten { keycodes: write_readback },
            }),
        ),
        (
            "keymap_write_request_empty",
            Req(55, Request::KeymapWrite { entries: heapless::Vec::new() }),
        ),
        (
            "keymap_write_response_empty",
            Resp(Response {
                request_id: 55,
                command: Command::KeymapWrite,
                status: Status::Ok,
                payload: ResponsePayload::KeymapWritten { keycodes: heapless::Vec::new() },
            }),
        ),
        (
            "keymap_write_response_out_of_range",
            empty_err(56, Command::KeymapWrite, Status::OutOfRange),
        ),
        (
            "keymap_write_response_capacity_exceeded",
            empty_err(57, Command::KeymapWrite, Status::CapacityExceeded),
        ),
    ]
}

fn golden_doc() -> Value {
    json!({
        "protocol": { "major": 1, "minor": 2 },
        "generatedBy": "crates/glove80-host-protocol tests/golden_v12.rs (GLOVE80_WRITE_VECTORS=1 cargo test --test golden_v12)",
        "messages": message_vectors(&messages()),
    })
}

#[test]
fn golden_file_matches_generator() {
    let doc = golden_doc();
    if common::maybe_write(FILE_NAME, &doc) {
        return;
    }
    assert_eq!(
        doc,
        common::load_file(FILE_NAME),
        "vector file is stale; regenerate with GLOVE80_WRITE_VECTORS=1 cargo test --test golden_v12"
    );
}

#[test]
fn golden_messages_decode() {
    common::assert_messages_decode(&common::load_file(FILE_NAME), &messages());
}
