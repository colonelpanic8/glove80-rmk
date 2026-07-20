//! Shared helpers for the golden-vector suites (v1 and v1.1).
//!
//! The canonical JSON representation here is mirrored by the TypeScript
//! codec/tests (`ui/src/lib/host-protocol.ts` / `host-protocol.test.ts`).

// Each test binary compiles this module separately and uses a subset of it.
#![allow(dead_code)]

use std::path::PathBuf;

use glove80_host_protocol::{
    encode_request, encode_response, feature, BootTarget, CellWrite, Command, Effect, EffectKind,
    Request, Response, ResponsePayload, Status, MAX_MESSAGE_LEN,
};
use serde_json::{json, Map, Value};

pub fn vector_path(file_name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("vectors").join(file_name)
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn unhex(s: &str) -> Vec<u8> {
    assert!(s.len() % 2 == 0, "odd hex length");
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

pub fn command_name(c: Command) -> &'static str {
    match c {
        Command::GetCapabilities => "getCapabilities",
        Command::Ping => "ping",
        Command::GetVersion => "getVersion",
        Command::SetCells => "setCells",
        Command::UnsetCells => "unsetCells",
        Command::ClearOverlay => "clearOverlay",
        Command::ReadOverlay => "readOverlay",
        Command::ReplaceOverlay => "replaceOverlay",
        Command::GetBrightness => "getBrightness",
        Command::SetBrightness => "setBrightness",
        Command::GetToggle => "getToggle",
        Command::SetToggle => "setToggle",
        Command::ConfigBegin => "configBegin",
        Command::ConfigData => "configData",
        Command::ConfigCommit => "configCommit",
        Command::ConfigAbort => "configAbort",
        Command::ConfigRead => "configRead",
        Command::KeymapRead => "keymapRead",
        Command::KeymapWrite => "keymapWrite",
        Command::EnterBootloader => "enterBootloader",
    }
}

pub fn status_name(s: Status) -> &'static str {
    match s {
        Status::Ok => "ok",
        Status::UnknownCommand => "unknownCommand",
        Status::Malformed => "malformed",
        Status::OutOfRange => "outOfRange",
        Status::CapacityExceeded => "capacityExceeded",
        Status::PartialApply => "partialApply",
        Status::Busy => "busy",
        Status::UnknownToggle => "unknownToggle",
        Status::BadMagic => "badMagic",
        Status::UnsupportedVersion => "unsupportedVersion",
        Status::NoSession => "noSession",
        Status::BadOffset => "badOffset",
        Status::ConfigIncomplete => "configIncomplete",
        Status::CrcMismatch => "crcMismatch",
        Status::InvalidConfig => "invalidConfig",
    }
}

pub fn effect_json(e: &Effect) -> Value {
    let kind = match e.kind {
        EffectKind::Solid => "solid",
        EffectKind::Blink => "blink",
        EffectKind::Breathe => "breathe",
    };
    json!({
        "kind": kind,
        "r": e.r,
        "g": e.g,
        "b": e.b,
        "periodMs": e.period_ms,
        "phaseMs": e.phase_ms,
        "dutyPercent": e.duty_percent,
    })
}

pub fn cells_json(cells: &[CellWrite]) -> Value {
    Value::Array(
        cells
            .iter()
            .map(|c| json!({ "key": c.key, "effect": effect_json(&c.effect) }))
            .collect(),
    )
}

pub fn request_json(req: &Request) -> Value {
    let mut obj = Map::new();
    obj.insert("command".into(), command_name(req.command()).into());
    match req {
        Request::GetCapabilities { client_major, client_minor } => {
            obj.insert("clientMajor".into(), (*client_major).into());
            obj.insert("clientMinor".into(), (*client_minor).into());
        }
        Request::Ping { data } => {
            obj.insert("dataHex".into(), hex(data).into());
        }
        Request::GetVersion => {}
        Request::SetCells { ttl_ms, cells } | Request::ReplaceOverlay { ttl_ms, cells } => {
            obj.insert("ttlMs".into(), (*ttl_ms).into());
            obj.insert("cells".into(), cells_json(cells));
        }
        Request::UnsetCells { keys } => {
            obj.insert("keys".into(), Value::Array(keys.iter().map(|k| (*k).into()).collect()));
        }
        Request::ClearOverlay | Request::ReadOverlay | Request::GetBrightness => {}
        Request::SetBrightness { level } => {
            obj.insert("level".into(), (*level).into());
        }
        Request::GetToggle { id } => {
            obj.insert("id".into(), (*id).into());
        }
        Request::SetToggle { id, state } => {
            obj.insert("id".into(), (*id).into());
            obj.insert("state".into(), (*state).into());
        }
        Request::ConfigBegin { total_len, blob_crc32 } => {
            obj.insert("totalLen".into(), (*total_len).into());
            obj.insert("blobCrc32".into(), (*blob_crc32).into());
        }
        Request::ConfigData { offset, data } => {
            obj.insert("offset".into(), (*offset).into());
            obj.insert("dataHex".into(), hex(data).into());
        }
        Request::ConfigCommit | Request::ConfigAbort => {}
        Request::ConfigRead { offset, max_len } => {
            obj.insert("offset".into(), (*offset).into());
            obj.insert("maxLen".into(), (*max_len).into());
        }
        Request::KeymapRead { layer, start_key, max_count } => {
            obj.insert("layer".into(), (*layer).into());
            obj.insert("startKey".into(), (*start_key).into());
            obj.insert("maxCount".into(), (*max_count).into());
        }
        Request::KeymapWrite { entries } => {
            obj.insert(
                "entries".into(),
                Value::Array(
                    entries
                        .iter()
                        .map(|e| json!({ "layer": e.layer, "key": e.key, "keycode": e.keycode }))
                        .collect(),
                ),
            );
        }
        Request::EnterBootloader { magic, target } => {
            obj.insert("magic".into(), (*magic).into());
            obj.insert(
                "target".into(),
                match target {
                    BootTarget::Central => "central",
                    BootTarget::Peripheral => "peripheral",
                }
                .into(),
            );
        }
    }
    Value::Object(obj)
}

pub fn payload_json(p: &ResponsePayload) -> Value {
    match p {
        ResponsePayload::Empty => json!({ "type": "empty" }),
        ResponsePayload::Capabilities(c) => {
            let mut obj = Map::new();
            obj.insert("type".into(), "capabilities".into());
            obj.insert("protocolMajor".into(), c.protocol_major.into());
            obj.insert("protocolMinor".into(), c.protocol_minor.into());
            obj.insert("ledCountLeft".into(), c.led_count_left.into());
            obj.insert("ledCountRight".into(), c.led_count_right.into());
            obj.insert("layerCapacity".into(), c.layer_capacity.into());
            obj.insert("maxCellsPerOp".into(), c.max_cells_per_op.into());
            obj.insert("effectMask".into(), c.effect_mask.into());
            obj.insert("overlayCellCapacity".into(), c.overlay_cell_capacity.into());
            obj.insert("maxMessageLen".into(), c.max_message_len.into());
            obj.insert("featureBits".into(), c.feature_bits.into());
            // Mirrors the wire: present iff the persistent-config bit is set.
            if c.feature_bits & feature::PERSISTENT_CONFIG != 0 {
                obj.insert("maxConfigBlobLen".into(), c.max_config_blob_len.into());
            }
            // Mirrors the wire: present iff the keymap bit is set (v1.2).
            if c.feature_bits & feature::KEYMAP != 0 {
                obj.insert("keymapRows".into(), c.keymap_rows.into());
                obj.insert("keymapCols".into(), c.keymap_cols.into());
                obj.insert("maxKeymapEntriesPerOp".into(), c.max_keymap_entries_per_op.into());
            }
            Value::Object(obj)
        }
        ResponsePayload::Echo { data } => json!({ "type": "echo", "dataHex": hex(data) }),
        ResponsePayload::Version(v) => {
            let half = |h: &glove80_host_protocol::HalfVersion| {
                json!({
                    "present": h.present,
                    "fwMajor": h.fw_major,
                    "fwMinor": h.fw_minor,
                    "fwPatch": h.fw_patch,
                    // Raw 8 wire bytes (ASCII short hash, zero-padded).
                    "gitHashHex": hex(&h.git_hash),
                    "dirty": h.dirty,
                })
            };
            json!({
                "type": "version",
                "central": half(&v.central),
                "peripheral": half(&v.peripheral),
                "halvesMismatch": v.halves_mismatch,
            })
        }
        ResponsePayload::OverlayAck { pending_keys } => json!({
            "type": "overlayAck",
            "pendingKeys": pending_keys.iter().copied().collect::<Vec<u8>>(),
        }),
        ResponsePayload::OverlayState { cells } => json!({
            "type": "overlayState",
            "cells": cells
                .iter()
                .map(|c| json!({
                    "key": c.key,
                    "effect": effect_json(&c.effect),
                    "remainingTtlMs": c.remaining_ttl_ms,
                }))
                .collect::<Vec<Value>>(),
        }),
        ResponsePayload::Brightness { level } => json!({ "type": "brightness", "level": level }),
        ResponsePayload::Toggle { id, state } => {
            json!({ "type": "toggle", "id": id, "state": state })
        }
        ResponsePayload::ConfigData { total_len, data } => json!({
            "type": "configData",
            "totalLen": total_len,
            "dataHex": hex(data),
        }),
        ResponsePayload::KeymapActions { layer, start_key, keycodes } => json!({
            "type": "keymapActions",
            "layer": layer,
            "startKey": start_key,
            "keycodes": keycodes.iter().copied().collect::<Vec<u16>>(),
        }),
        ResponsePayload::KeymapWritten { keycodes } => json!({
            "type": "keymapWritten",
            "keycodes": keycodes.iter().copied().collect::<Vec<u16>>(),
        }),
    }
}

pub fn response_json(resp: &Response) -> Value {
    json!({
        "command": command_name(resp.command),
        "status": status_name(resp.status),
        "payload": payload_json(&resp.payload),
    })
}

pub enum Message {
    Req(u8, Request),
    Resp(Response),
}

pub fn heapless_bytes<const N: usize>(data: &[u8]) -> heapless::Vec<u8, N> {
    heapless::Vec::from_slice(data).unwrap()
}

pub fn encode_message(m: &Message) -> Vec<u8> {
    let mut buf = [0u8; MAX_MESSAGE_LEN];
    let len = match m {
        Message::Req(id, req) => encode_request(*id, req, &mut buf).unwrap(),
        Message::Resp(resp) => encode_response(resp, &mut buf).unwrap(),
    };
    buf[..len].to_vec()
}

pub fn message_vectors(messages: &[(&'static str, Message)]) -> Vec<Value> {
    messages
        .iter()
        .map(|(name, m)| {
            let bytes = encode_message(m);
            match m {
                Message::Req(id, req) => json!({
                    "name": name,
                    "kind": "request",
                    "requestId": id,
                    "message": request_json(req),
                    "hex": hex(&bytes),
                }),
                Message::Resp(resp) => json!({
                    "name": name,
                    "kind": "response",
                    "requestId": resp.request_id,
                    "message": response_json(resp),
                    "hex": hex(&bytes),
                }),
            }
        })
        .collect()
}

pub fn load_file(file_name: &str) -> Value {
    let path = vector_path(file_name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap()
}

/// With `GLOVE80_WRITE_VECTORS=1`, (re)write the vector file and return
/// true; otherwise return false so the caller compares against the file.
pub fn maybe_write(file_name: &str, doc: &Value) -> bool {
    if std::env::var("GLOVE80_WRITE_VECTORS").is_err() {
        return false;
    }
    let path = vector_path(file_name);
    std::fs::write(&path, serde_json::to_string_pretty(doc).unwrap() + "\n").unwrap();
    true
}

/// Decode every message vector and check it equals its construction.
pub fn assert_messages_decode(file: &Value, constructed: &[(&'static str, Message)]) {
    use glove80_host_protocol::{decode_request, decode_response};
    for entry in file["messages"].as_array().unwrap() {
        let name = entry["name"].as_str().unwrap();
        let bytes = unhex(entry["hex"].as_str().unwrap());
        let (_, expected) = constructed
            .iter()
            .find(|(n, _)| *n == name)
            .unwrap_or_else(|| panic!("vector {name} not constructed in this suite"));
        match expected {
            Message::Req(id, req) => {
                let (decoded_id, decoded) =
                    decode_request(&bytes).unwrap_or_else(|e| panic!("decode {name}: {e:?}"));
                assert_eq!(decoded_id, *id, "{name}");
                assert_eq!(&decoded, req, "{name}");
            }
            Message::Resp(resp) => {
                let decoded =
                    decode_response(&bytes).unwrap_or_else(|e| panic!("decode {name}: {e:?}"));
                assert_eq!(&decoded, resp, "{name}");
            }
        }
    }
}
