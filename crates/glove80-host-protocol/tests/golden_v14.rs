//! Golden vector suite for the protocol v1.4 conditional-lighting config
//! gates. Generates/checks `crates/glove80-host-protocol/vectors/host-protocol-v1.4.json`.
//! All older vector files are frozen and must never be regenerated.

mod common;

use common::Message;
use glove80_host_protocol::{
    crc32, decode_lighting_config, encode_lighting_config, Capabilities, CellWrite, Command,
    ConfigActivation, ConfigError, ConfigGate, ConfigRecord, Effect, LightingConfig, Response,
    ResponsePayload, Status, CONFIG_HEADER_LEN, CONFIG_MAGIC, CONFIG_VERSION, MAX_CONFIG_BLOB_LEN,
    MAX_MESSAGE_LEN,
};
use serde_json::{json, Value};

const FILE_NAME: &str = "host-protocol-v1.4.json";

fn gated_config() -> LightingConfig {
    let mut records = heapless::Vec::new();
    let gates = [
        ConfigGate::LayerActive(7),
        ConfigGate::Toggle(31),
        ConfigGate::UsbConnected,
        ConfigGate::Charging,
        ConfigGate::SplitLinkUp,
    ];
    for (key, gate) in gates.into_iter().enumerate() {
        records
            .push(ConfigRecord {
                activation: if key == 0 {
                    ConfigActivation::LayerActive(2)
                } else {
                    ConfigActivation::Always
                },
                gate: Some(gate),
                cells: heapless::Vec::from_slice(&[CellWrite {
                    key: key as u8,
                    effect: Effect::solid((key as u8 + 1) * 10, 20, 30),
                }])
                .unwrap(),
            })
            .unwrap();
    }
    LightingConfig {
        toggle_persist_mask: 0,
        toggle_initial_state: 0,
        records,
    }
}

fn encode_config(config: &LightingConfig) -> Vec<u8> {
    let mut buf = [0u8; MAX_CONFIG_BLOB_LEN];
    let len = encode_lighting_config(config, &mut buf).unwrap();
    buf[..len].to_vec()
}

fn gate_json(gate: ConfigGate) -> Value {
    match gate {
        ConfigGate::LayerActive(layer) => json!({ "kind": "layerActive", "layer": layer }),
        ConfigGate::Toggle(id) => json!({ "kind": "toggle", "id": id }),
        ConfigGate::UsbConnected => json!({ "kind": "usbConnected" }),
        ConfigGate::Charging => json!({ "kind": "charging" }),
        ConfigGate::SplitLinkUp => json!({ "kind": "splitLinkUp" }),
    }
}

fn config_json(config: &LightingConfig) -> Value {
    json!({
        "togglePersistMask": config.toggle_persist_mask,
        "toggleInitialState": config.toggle_initial_state,
        "records": config.records.iter().map(|record| {
            let activation = match record.activation {
                ConfigActivation::Always => json!({ "kind": "always" }),
                ConfigActivation::LayerActive(layer) => {
                    json!({ "kind": "layerActive", "layer": layer })
                }
                ConfigActivation::Toggle(id) => json!({ "kind": "toggle", "id": id }),
            };
            json!({
                "activation": activation,
                "gate": record.gate.map(gate_json),
                "cells": common::cells_json(&record.cells),
            })
        }).collect::<Vec<_>>(),
    })
}

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

fn mutated_gate(kind: u8, arg: u8) -> Vec<u8> {
    let good = encode_config(&gated_config());
    let mut body = good[CONFIG_HEADER_LEN..].to_vec();
    body[14] = kind;
    body[15] = arg;
    reheader(&body)
}

fn invalid_configs() -> Vec<(&'static str, Vec<u8>, ConfigError)> {
    vec![
        (
            "unknown_gate",
            mutated_gate(6, 0),
            ConfigError::UnknownGate(6),
        ),
        (
            "gate_layer_out_of_range",
            mutated_gate(1, 8),
            ConfigError::GateLayerOutOfRange(8),
        ),
        (
            "gate_toggle_out_of_range",
            mutated_gate(2, 32),
            ConfigError::GateToggleOutOfRange(32),
        ),
        (
            "usb_gate_nonzero_arg",
            mutated_gate(3, 1),
            ConfigError::GateArgNonZero(1),
        ),
        (
            "charging_gate_nonzero_arg",
            mutated_gate(4, 1),
            ConfigError::GateArgNonZero(1),
        ),
        (
            "split_link_gate_nonzero_arg",
            mutated_gate(5, 1),
            ConfigError::GateArgNonZero(1),
        ),
    ]
}

fn error_name(error: ConfigError) -> &'static str {
    match error {
        ConfigError::UnknownGate(_) => "unknownGate",
        ConfigError::GateLayerOutOfRange(_) => "gateLayerOutOfRange",
        ConfigError::GateToggleOutOfRange(_) => "gateToggleOutOfRange",
        ConfigError::GateArgNonZero(_) => "gateArgNonZero",
        _ => panic!("not a v1.4 gate validation error: {error:?}"),
    }
}

fn capabilities_message() -> Message {
    Message::Resp(Response {
        request_id: 80,
        command: Command::GetCapabilities,
        status: Status::Ok,
        payload: ResponsePayload::Capabilities(Capabilities {
            protocol_major: 1,
            protocol_minor: 4,
            led_count_left: 40,
            led_count_right: 40,
            layer_capacity: 8,
            max_cells_per_op: 80,
            effect_mask: 0b0000_0111,
            overlay_cell_capacity: 80,
            max_message_len: MAX_MESSAGE_LEN as u16,
            feature_bits: 0x3FF,
            max_config_blob_len: MAX_CONFIG_BLOB_LEN as u32,
            keymap_rows: 6,
            keymap_cols: 14,
            max_keymap_entries_per_op: 84,
        }),
    })
}

fn golden_doc() -> Value {
    let config = gated_config();
    json!({
        "protocol": { "major": 1, "minor": 4 },
        "generatedBy": "crates/glove80-host-protocol tests/golden_v14.rs (GLOVE80_WRITE_VECTORS=1 cargo test --test golden_v14)",
        "messages": common::message_vectors(&[("get_capabilities_response_config_gates", capabilities_message())]),
        "configs": [{
            "name": "all_gate_kinds",
            "config": config_json(&config),
            "hex": common::hex(&encode_config(&config)),
        }],
        "invalidConfigs": invalid_configs().iter().map(|(name, bytes, error)| json!({
            "name": name,
            "hex": common::hex(bytes),
            "error": error_name(*error),
        })).collect::<Vec<_>>(),
    })
}

#[test]
fn golden_file_matches_generator() {
    let doc = golden_doc();
    if common::maybe_write(FILE_NAME, &doc) {
        return;
    }
    assert_eq!(doc, common::load_file(FILE_NAME));
}

#[test]
fn golden_message_decodes() {
    common::assert_messages_decode(
        &common::load_file(FILE_NAME),
        &[(
            "get_capabilities_response_config_gates",
            capabilities_message(),
        )],
    );
}

#[test]
fn golden_config_roundtrips_and_invalid_cases_reject() {
    let file = common::load_file(FILE_NAME);
    let config = gated_config();
    let bytes = common::unhex(file["configs"][0]["hex"].as_str().unwrap());
    assert_eq!(decode_lighting_config(&bytes), Ok(config));
    for entry in file["invalidConfigs"].as_array().unwrap() {
        let name = entry["name"].as_str().unwrap();
        let bytes = common::unhex(entry["hex"].as_str().unwrap());
        let expected = invalid_configs()
            .into_iter()
            .find(|(candidate, _, _)| *candidate == name)
            .unwrap()
            .2;
        assert_eq!(decode_lighting_config(&bytes), Err(expected), "{name}");
    }
}
