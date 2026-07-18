#define DT_DRV_COMPAT zmk_behavior_host_lighting

#include <zephyr/device.h>
#include <zephyr/devicetree.h>

#include <drivers/behavior.h>
#include <zmk/behavior.h>
#include <zmk/host_lighting.h>
#include <zmk/rgb_underglow.h>

#if DT_HAS_COMPAT_STATUS_OKAY(DT_DRV_COMPAT)

static int on_pressed(struct zmk_behavior_binding *binding,
                      struct zmk_behavior_binding_event event) {
    const uint8_t command = ZMK_HOST_LIGHTING_COMMAND(binding->param1);
    const uint32_t timeout_ms = ZMK_HOST_LIGHTING_TIMEOUT(binding->param1);

    if (command == ZMK_HOST_LIGHTING_CMD_CLEAR) {
        return zmk_rgb_underglow_host_clear();
    }

    if (command == ZMK_HOST_LIGHTING_CMD_REPLACE) {
        return zmk_rgb_underglow_host_replace(timeout_ms);
    }

    const struct zmk_rgb_underglow_host_pixel update = {
        .index = command,
        .r = (binding->param2 >> 16) & 0xff,
        .g = (binding->param2 >> 8) & 0xff,
        .b = binding->param2 & 0xff,
    };

    return zmk_rgb_underglow_host_update(&update, 1, timeout_ms);
}

static int on_released(struct zmk_behavior_binding *binding,
                       struct zmk_behavior_binding_event event) {
    return ZMK_BEHAVIOR_OPAQUE;
}

static const struct behavior_driver_api host_lighting_driver_api = {
    .binding_pressed = on_pressed,
    .binding_released = on_released,
    .locality = BEHAVIOR_LOCALITY_EVENT_SOURCE,
};

BEHAVIOR_DT_INST_DEFINE(0, NULL, NULL, NULL, NULL, POST_KERNEL,
                        CONFIG_KERNEL_INIT_PRIORITY_DEFAULT, &host_lighting_driver_api);

#endif
