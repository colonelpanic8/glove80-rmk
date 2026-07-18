/*
 * Copyright (c) 2026 The ZMK Contributors
 * SPDX-License-Identifier: MIT
 */

#pragma once

#include <zephyr/devicetree.h>
#include <zephyr/storage/flash_map.h>

/*
 * Boards may expose dedicated runtime-configuration storage through the
 * zmk,runtime-config-partition chosen property. Keeping this optional lets
 * generic ZMK and settings-reset builds continue to work on other boards.
 */
#if DT_HAS_CHOSEN(zmk_runtime_config_partition)
#define ZMK_RUNTIME_CONFIG_PARTITION_EXISTS 1
#define ZMK_RUNTIME_CONFIG_PARTITION_NODE DT_CHOSEN(zmk_runtime_config_partition)
#define ZMK_RUNTIME_CONFIG_PARTITION_ID                                                   \
    DT_FIXED_PARTITION_ID(ZMK_RUNTIME_CONFIG_PARTITION_NODE)
#else
#define ZMK_RUNTIME_CONFIG_PARTITION_EXISTS 0
#endif
