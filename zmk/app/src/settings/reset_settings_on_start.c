/*
 * Copyright (c) 2023 The ZMK Contributors
 *
 * SPDX-License-Identifier: MIT
 */

#include <zephyr/init.h>
#include <zephyr/logging/log.h>
#include <zephyr/storage/flash_map.h>

#include <zmk/runtime_config/partition.h>
#include <zmk/settings.h>

LOG_MODULE_DECLARE(zmk, CONFIG_ZMK_LOG_LEVEL);

static int zmk_settings_reset_on_start(void) {
    int settings_rc = zmk_settings_erase();

#if ZMK_RUNTIME_CONFIG_PARTITION_EXISTS
    LOG_INF("Erasing runtime configuration flash partition");

    const struct flash_area *runtime_config_area;
    int runtime_config_rc =
        flash_area_open(ZMK_RUNTIME_CONFIG_PARTITION_ID, &runtime_config_area);
    if (runtime_config_rc) {
        LOG_ERR("Failed to open runtime configuration flash: %d", runtime_config_rc);
    } else {
        runtime_config_rc = flash_area_erase(runtime_config_area, 0, runtime_config_area->fa_size);
        if (runtime_config_rc) {
            LOG_ERR("Failed to erase runtime configuration flash: %d", runtime_config_rc);
        }
        flash_area_close(runtime_config_area);
    }

    if (!settings_rc) {
        return runtime_config_rc;
    }
#endif

    return settings_rc;
}

// Reset after the kernel is initialized but before any application code to
// ensure settings are cleared before anything tries to use them.
SYS_INIT(zmk_settings_reset_on_start, POST_KERNEL,
         CONFIG_ZMK_SETTINGS_RESET_ON_START_INIT_PRIORITY);
