#pragma once

#include <stdint.h>

#define ZMK_HOST_LIGHTING_PROTOCOL_VERSION 1
#define ZMK_HOST_LIGHTING_BEHAVIOR_NAME "hostled"

#define ZMK_HOST_LIGHTING_CMD_REPLACE 0xfe
#define ZMK_HOST_LIGHTING_CMD_CLEAR 0xff

#define ZMK_HOST_LIGHTING_PACK_COMMAND(command, timeout_ms)                                       \
    (((uint32_t)(timeout_ms) << 8) | ((command) & 0xff))
#define ZMK_HOST_LIGHTING_COMMAND(value) ((value) & 0xff)
#define ZMK_HOST_LIGHTING_TIMEOUT(value) ((value) >> 8)
