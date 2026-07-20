//! Golden vector suite for the protocol v1.1 additions (persistent config).
//!
//! Generates/checks `crates/glove80-host-protocol/vectors/host-protocol-v1.1.json`, consumed by
//! this suite and the TypeScript suite. Regenerate with
//! `GLOVE80_WRITE_VECTORS=1 cargo test --test golden_v11`. The v1.0 vectors
//! (`host-protocol-v1.json`, `golden.rs`) are frozen and never change.
//!
//! These vectors are **frozen at v1.1**: version numbers and feature bits
//! below are literals (not the crate constants) so that later minor bumps
//! can never change a v1.1 byte. v1.2 vectors live in `golden_v12.rs` /
//! `host-protocol-v1.2.json`.

mod common;

use common::{message_vectors, Message};
use glove80_host_protocol::{
    crc32, decode_lighting_config, encode_lighting_config, Capabilities, CellWrite, Command,
    ConfigActivation, ConfigError, ConfigRecord, Effect, LightingConfig, Request, Response,
    ResponsePayload, Status, CONFIG_HEADER_LEN, CONFIG_MAGIC, CONFIG_VERSION,
    MAX_CELLS_PER_RECORD, MAX_CONFIG_BLOB_LEN, MAX_CONFIG_RECORDS, MAX_MESSAGE_LEN,
};
use serde_json::{json, Value};

const FILE_NAME: &str = "host-protocol-v1.1.json";

// --- configs --------------------------------------------------------------

fn empty_config() -> LightingConfig {
    LightingConfig::default()
}

fn sample_config() -> LightingConfig {
    let mut records = heapless::Vec::new();
    records
        .push(ConfigRecord {
            activation: ConfigActivation::Always,
            gate: None,
            cells: heapless::Vec::from_slice(&[
                CellWrite { key: 0, effect: Effect::solid(10, 20, 30) },
                CellWrite { key: 79, effect: Effect::blink(255, 0, 64, 1000, 250, 50) },
            ])
            .unwrap(),
        })
        .unwrap();
    records
        .push(ConfigRecord {
            activation: ConfigActivation::LayerActive(3),
            gate: None,
            cells: heapless::Vec::from_slice(&[CellWrite {
                key: 40,
                effect: Effect::breathe(16, 32, 48, 3000, 0),
            }])
            .unwrap(),
        })
        .unwrap();
    records
        .push(ConfigRecord {
            activation: ConfigActivation::Toggle(7),
            gate: None,
            cells: heapless::Vec::new(),
        })
        .unwrap();
    LightingConfig {
        toggle_persist_mask: 0x0000_0080,
        toggle_initial_state: 0x0000_0081,
        records,
    }
}

fn max_config() -> LightingConfig {
    let mut records = heapless::Vec::new();
    for i in 0..MAX_CONFIG_RECORDS {
        let activation = match i % 3 {
            0 => ConfigActivation::Always,
            1 => ConfigActivation::LayerActive((i % 8) as u8),
            _ => ConfigActivation::Toggle((i % 32) as u8),
        };
        let mut cells = heapless::Vec::new();
        for k in 0..MAX_CELLS_PER_RECORD {
            cells
                .push(CellWrite {
                    key: k as u8,
                    effect: Effect::blink((i * 16) as u8, k as u8, 200, 750, 125, 40),
                })
                .unwrap();
        }
        records.push(ConfigRecord { activation, gate: None, cells }).unwrap();
    }
    LightingConfig { toggle_persist_mask: u32::MAX, toggle_initial_state: 0x5555_5555, records }
}

fn encode_config(config: &LightingConfig) -> Vec<u8> {
    let mut buf = [0u8; MAX_CONFIG_BLOB_LEN];
    let len = encode_lighting_config(config, &mut buf).unwrap();
    buf[..len].to_vec()
}

fn activation_json(a: ConfigActivation) -> Value {
    match a {
        ConfigActivation::Always => json!({ "kind": "always" }),
        ConfigActivation::LayerActive(layer) => json!({ "kind": "layerActive", "layer": layer }),
        ConfigActivation::Toggle(id) => json!({ "kind": "toggle", "id": id }),
    }
}

fn config_json(config: &LightingConfig) -> Value {
    json!({
        "togglePersistMask": config.toggle_persist_mask,
        "toggleInitialState": config.toggle_initial_state,
        "records": config
            .records
            .iter()
            .map(|rec| json!({
                "activation": activation_json(rec.activation),
                "cells": common::cells_json(&rec.cells),
            }))
            .collect::<Vec<Value>>(),
    })
}

fn config_vectors() -> Vec<Value> {
    [
        ("empty_config", empty_config()),
        ("sample_config", sample_config()),
        ("max_config", max_config()),
    ]
    .iter()
    .map(|(name, config)| {
        json!({
            "name": name,
            "config": config_json(config),
            "hex": common::hex(&encode_config(config)),
        })
    })
    .collect()
}

// --- invalid configs ------------------------------------------------------

fn error_name(e: ConfigError) -> &'static str {
    match e {
        ConfigError::Truncated => "truncated",
        ConfigError::LengthMismatch => "lengthMismatch",
        ConfigError::BadMagic(_) => "badMagic",
        ConfigError::UnsupportedVersion(_) => "unsupportedVersion",
        ConfigError::CrcMismatch { .. } => "crcMismatch",
        ConfigError::TooManyRecords(_) => "tooManyRecords",
        ConfigError::TooManyCells(_) => "tooManyCells",
        ConfigError::UnknownActivation(_) => "unknownActivation",
        ConfigError::LayerOutOfRange(_) => "layerOutOfRange",
        ConfigError::ToggleOutOfRange(_) => "toggleOutOfRange",
        ConfigError::KeyOutOfRange(_) => "keyOutOfRange",
        ConfigError::DuplicateKey(_) => "duplicateKey",
        ConfigError::UnknownEffectKind(_) => "unknownEffectKind",
        ConfigError::UnknownGate(_) => "unknownGate",
        ConfigError::GateLayerOutOfRange(_) => "gateLayerOutOfRange",
        ConfigError::GateToggleOutOfRange(_) => "gateToggleOutOfRange",
        ConfigError::GateArgNonZero(_) => "gateArgNonZero",
    }
}

/// Wrap arbitrary body bytes in a valid header (correct len + CRC).
fn reheader(body: &[u8]) -> Vec<u8> {
    let mut blob = Vec::new();
    blob.extend_from_slice(&CONFIG_MAGIC.to_le_bytes());
    blob.extend_from_slice(&CONFIG_VERSION.to_le_bytes());
    blob.extend_from_slice(&[0, 0]);
    blob.extend_from_slice(&(body.len() as u32).to_le_bytes());
    blob.extend_from_slice(&crc32(body).to_le_bytes());
    blob.extend_from_slice(body);
    blob
}

fn mutated_body(mutate: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
    let good = encode_config(&sample_config());
    let mut body = good[CONFIG_HEADER_LEN..].to_vec();
    mutate(&mut body);
    reheader(&body)
}

/// `(name, blob bytes, expected decode error)`. Body offsets: 12-byte body
/// prefix, record 0 header at 12 (activation, arg, reserved u16,
/// cell_count), record 0 cells at 17 (11 bytes each).
fn invalid_configs() -> Vec<(&'static str, Vec<u8>, ConfigError)> {
    let good = encode_config(&sample_config());
    let mut bad_magic = good.clone();
    bad_magic[0] = 0x00;
    let mut bad_version = good.clone();
    bad_version[4] = 2;
    let mut bad_crc = good.clone();
    bad_crc[12] ^= 0x01; // corrupt the stored body_crc32
    let mut bad_body_len = good.clone();
    bad_body_len[8] = bad_body_len[8].wrapping_add(1);
    vec![
        ("bad_magic", bad_magic, ConfigError::BadMagic(0x4C30_3800)),
        ("bad_version", bad_version, ConfigError::UnsupportedVersion(2)),
        (
            "bad_crc",
            bad_crc.clone(),
            ConfigError::CrcMismatch {
                expected: u32::from_le_bytes(bad_crc[12..16].try_into().unwrap()),
                actual: crc32(&bad_crc[CONFIG_HEADER_LEN..]),
            },
        ),
        ("body_len_mismatch", bad_body_len, ConfigError::LengthMismatch),
        ("truncated_header", good[..10].to_vec(), ConfigError::Truncated),
        (
            "truncated_body",
            mutated_body(|b| {
                let n = b.len() - 4;
                b.truncate(n);
            }),
            ConfigError::Truncated,
        ),
        ("trailing_bytes", mutated_body(|b| b.push(0)), ConfigError::LengthMismatch),
        (
            "too_many_records",
            mutated_body(|b| b[0] = 17),
            ConfigError::TooManyRecords(17),
        ),
        (
            "unknown_activation",
            mutated_body(|b| b[12] = 3),
            ConfigError::UnknownActivation(3),
        ),
        (
            "layer_out_of_range",
            mutated_body(|b| {
                b[12] = 1;
                b[13] = 8;
            }),
            ConfigError::LayerOutOfRange(8),
        ),
        (
            "toggle_out_of_range",
            mutated_body(|b| {
                b[12] = 2;
                b[13] = 32;
            }),
            ConfigError::ToggleOutOfRange(32),
        ),
        ("too_many_cells", mutated_body(|b| b[16] = 41), ConfigError::TooManyCells(41)),
        ("key_out_of_range", mutated_body(|b| b[17] = 80), ConfigError::KeyOutOfRange(80)),
        (
            "duplicate_key",
            mutated_body(|b| b[28] = b[17]),
            ConfigError::DuplicateKey(0),
        ),
        (
            "unknown_effect_kind",
            mutated_body(|b| b[18] = 9),
            ConfigError::UnknownEffectKind(9),
        ),
    ]
}

fn invalid_config_vectors() -> Vec<Value> {
    invalid_configs()
        .iter()
        .map(|(name, bytes, error)| {
            json!({ "name": name, "hex": common::hex(bytes), "error": error_name(*error) })
        })
        .collect()
}

// --- messages -------------------------------------------------------------

fn messages() -> Vec<(&'static str, Message)> {
    use Message::{Req, Resp};

    let blob = encode_config(&sample_config());
    let caps = Capabilities {
        // Frozen v1.1 stamp; see the module comment.
        protocol_major: 1,
        protocol_minor: 1,
        led_count_left: 40,
        led_count_right: 40,
        layer_capacity: 8,
        max_cells_per_op: 80,
        effect_mask: 0b0000_0111,
        overlay_cell_capacity: 80,
        max_message_len: MAX_MESSAGE_LEN as u16,
        feature_bits: 0x7F,
        max_config_blob_len: MAX_CONFIG_BLOB_LEN as u32,
        // Not on the wire: the keymap feature bit is clear at v1.1.
        keymap_rows: 0,
        keymap_cols: 0,
        max_keymap_entries_per_op: 0,
    };
    let empty_ok = |id: u8, command: Command| {
        Resp(Response { request_id: id, command, status: Status::Ok, payload: ResponsePayload::Empty })
    };
    let empty_err = |id: u8, command: Command, status: Status| {
        Resp(Response { request_id: id, command, status, payload: ResponsePayload::Empty })
    };

    vec![
        (
            "get_capabilities_response_persistent_config",
            Resp(Response {
                request_id: 30,
                command: Command::GetCapabilities,
                status: Status::Ok,
                payload: ResponsePayload::Capabilities(caps),
            }),
        ),
        (
            "config_begin_request",
            Req(31, Request::ConfigBegin {
                total_len: blob.len() as u32,
                blob_crc32: crc32(&blob),
            }),
        ),
        ("config_begin_response_ok", empty_ok(31, Command::ConfigBegin)),
        (
            "config_begin_response_capacity_exceeded",
            empty_err(32, Command::ConfigBegin, Status::CapacityExceeded),
        ),
        (
            "config_data_request",
            Req(33, Request::ConfigData {
                offset: 0,
                data: heapless::Vec::from_slice(&blob[..64]).unwrap(),
            }),
        ),
        ("config_data_response_ok", empty_ok(33, Command::ConfigData)),
        (
            "config_data_response_bad_offset",
            empty_err(34, Command::ConfigData, Status::BadOffset),
        ),
        (
            "config_data_response_no_session",
            empty_err(35, Command::ConfigData, Status::NoSession),
        ),
        ("config_commit_request", Req(36, Request::ConfigCommit)),
        ("config_commit_response_ok", empty_ok(36, Command::ConfigCommit)),
        (
            "config_commit_response_incomplete",
            empty_err(37, Command::ConfigCommit, Status::ConfigIncomplete),
        ),
        (
            "config_commit_response_crc_mismatch",
            empty_err(38, Command::ConfigCommit, Status::CrcMismatch),
        ),
        (
            "config_commit_response_invalid_config",
            empty_err(39, Command::ConfigCommit, Status::InvalidConfig),
        ),
        ("config_abort_request", Req(40, Request::ConfigAbort)),
        ("config_abort_response_ok", empty_ok(40, Command::ConfigAbort)),
        (
            "config_read_request",
            Req(41, Request::ConfigRead { offset: 0, max_len: 64 }),
        ),
        (
            "config_read_response",
            Resp(Response {
                request_id: 41,
                command: Command::ConfigRead,
                status: Status::Ok,
                payload: ResponsePayload::ConfigData {
                    total_len: blob.len() as u32,
                    data: heapless::Vec::from_slice(&blob[..64]).unwrap(),
                },
            }),
        ),
        (
            "config_read_response_end_of_blob",
            Resp(Response {
                request_id: 42,
                command: Command::ConfigRead,
                status: Status::Ok,
                payload: ResponsePayload::ConfigData {
                    total_len: blob.len() as u32,
                    data: heapless::Vec::new(),
                },
            }),
        ),
        (
            "config_read_response_out_of_range",
            empty_err(43, Command::ConfigRead, Status::OutOfRange),
        ),
    ]
}

fn golden_doc() -> Value {
    json!({
        // Frozen v1.1 stamp; see the module comment.
        "protocol": { "major": 1, "minor": 1 },
        "generatedBy": "crates/glove80-host-protocol tests/golden_v11.rs (GLOVE80_WRITE_VECTORS=1 cargo test --test golden_v11)",
        "messages": message_vectors(&messages()),
        "configs": config_vectors(),
        "invalidConfigs": invalid_config_vectors(),
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
        "vector file is stale; regenerate with GLOVE80_WRITE_VECTORS=1 cargo test --test golden_v11"
    );
}

#[test]
fn golden_messages_decode() {
    common::assert_messages_decode(&common::load_file(FILE_NAME), &messages());
}

#[test]
fn golden_configs_roundtrip() {
    let file = common::load_file(FILE_NAME);
    let constructed =
        [("empty_config", empty_config()), ("sample_config", sample_config()), ("max_config", max_config())];
    for entry in file["configs"].as_array().unwrap() {
        let name = entry["name"].as_str().unwrap();
        let bytes = common::unhex(entry["hex"].as_str().unwrap());
        let (_, expected) = constructed
            .iter()
            .find(|(n, _)| *n == name)
            .unwrap_or_else(|| panic!("config vector {name} not constructed in this suite"));
        assert_eq!(common::hex(&encode_config(expected)), entry["hex"].as_str().unwrap(), "{name}");
        let decoded =
            decode_lighting_config(&bytes).unwrap_or_else(|e| panic!("decode {name}: {e:?}"));
        assert_eq!(&decoded, expected, "{name}");
    }
}

#[test]
fn golden_invalid_configs_rejected() {
    let file = common::load_file(FILE_NAME);
    let constructed = invalid_configs();
    for entry in file["invalidConfigs"].as_array().unwrap() {
        let name = entry["name"].as_str().unwrap();
        let bytes = common::unhex(entry["hex"].as_str().unwrap());
        let (_, _, expected_error) = constructed
            .iter()
            .find(|(n, _, _)| *n == name)
            .unwrap_or_else(|| panic!("invalid-config vector {name} not constructed"));
        let err = decode_lighting_config(&bytes)
            .expect_err(&format!("{name}: expected {expected_error:?}"));
        assert_eq!(&err, expected_error, "{name}");
        assert_eq!(error_name(err), entry["error"].as_str().unwrap(), "{name}");
    }
}
