/*
 * Copyright (c) 2025 The ZMK Contributors
 *
 * SPDX-License-Identifier: MIT
 */

#pragma once

#include <zmk/hid_indicators_types.h>
#include <zmk/sensors.h>
#include <zephyr/sys/util.h>

enum zmk_split_transport_connections_status {
    ZMK_SPLIT_TRANSPORT_CONNECTIONS_STATUS_DISCONNECTED = 0,
    ZMK_SPLIT_TRANSPORT_CONNECTIONS_STATUS_SOME_CONNECTED,
    ZMK_SPLIT_TRANSPORT_CONNECTIONS_STATUS_ALL_CONNECTED,
};

struct zmk_split_transport_status {
    bool available;
    bool enabled;
    enum zmk_split_transport_connections_status connections;
};

typedef struct zmk_split_transport_status (*zmk_split_transport_get_status_t)(void);
typedef int (*zmk_split_transport_set_enabled_t)(bool enabled);

#define ZMK_SPLIT_HOST_LIGHTING_MAX_PIXELS 4
#define ZMK_SPLIT_HOST_LIGHTING_FLAG_REPLACE BIT(0)
#define ZMK_SPLIT_HOST_LIGHTING_FLAG_CLEAR BIT(1)

struct zmk_split_transport_host_lighting_pixel {
    uint8_t index;
    uint8_t r;
    uint8_t g;
    uint8_t b;
} __packed;

/* Exactly 20 bytes so one complete batch fits the default BLE ATT payload. */
struct zmk_split_transport_host_lighting_command {
    uint16_t timeout_ms;
    uint8_t pixel_count;
    uint8_t flags;
    struct zmk_split_transport_host_lighting_pixel pixels[ZMK_SPLIT_HOST_LIGHTING_MAX_PIXELS];
} __packed;

enum zmk_split_transport_peripheral_event_type {
    ZMK_SPLIT_TRANSPORT_PERIPHERAL_EVENT_TYPE_KEY_POSITION_EVENT,
    ZMK_SPLIT_TRANSPORT_PERIPHERAL_EVENT_TYPE_SENSOR_EVENT,
    ZMK_SPLIT_TRANSPORT_PERIPHERAL_EVENT_TYPE_INPUT_EVENT,
    ZMK_SPLIT_TRANSPORT_PERIPHERAL_EVENT_TYPE_BATTERY_EVENT,
};

struct zmk_split_transport_peripheral_event {
    enum zmk_split_transport_peripheral_event_type type;

    union {
        struct {
            uint8_t position;
            uint8_t pressed;
        } key_position_event;

        struct {
            struct zmk_sensor_channel_data channel_data;

            uint8_t sensor_index;
        } sensor_event;

        struct {
            uint8_t reg;
            uint8_t sync;
            uint8_t type;
            uint16_t code;
            int32_t value;
        } input_event;

        struct {
            uint8_t level;
        } battery_event;
    } data;
} __packed;

enum zmk_split_transport_central_command_type {
    ZMK_SPLIT_TRANSPORT_CENTRAL_CMD_TYPE_POLL_EVENTS,
    ZMK_SPLIT_TRANSPORT_CENTRAL_CMD_TYPE_INVOKE_BEHAVIOR,
    ZMK_SPLIT_TRANSPORT_CENTRAL_CMD_TYPE_SET_PHYSICAL_LAYOUT,
    ZMK_SPLIT_TRANSPORT_CENTRAL_CMD_TYPE_SET_HID_INDICATORS,
    ZMK_SPLIT_TRANSPORT_CENTRAL_CMD_TYPE_HOST_LIGHTING,
} __packed;

struct zmk_split_transport_central_command {
    enum zmk_split_transport_central_command_type type;

    union {
        struct {
            char behavior_dev[16];
            uint32_t param1, param2;
            uint32_t position;
            uint8_t event_source;
            uint8_t state;
        } invoke_behavior;

        struct {
            uint8_t layout_idx;
        } set_physical_layout;

        struct {
            zmk_hid_indicators_t indicators;
        } set_hid_indicators;

        struct zmk_split_transport_host_lighting_command host_lighting;
    } data;
} __packed;
