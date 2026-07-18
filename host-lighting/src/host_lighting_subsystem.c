#include <zephyr/kernel.h>
#include <zephyr/logging/log.h>

#include <zmk/behavior.h>
#include <zmk/host_lighting.h>
#include <zmk/rgb_underglow.h>
#include <zmk/split/central.h>
#include <zmk/studio/rpc.h>

LOG_MODULE_DECLARE(zmk_studio, CONFIG_ZMK_STUDIO_LOG_LEVEL);

ZMK_RPC_SUBSYSTEM(host_lighting)

#define HOST_LIGHTING_RESPONSE(type, ...) ZMK_RPC_RESPONSE(host_lighting, type, __VA_ARGS__)

static int send_peripheral_command(uint8_t command, uint32_t value, uint32_t timeout_ms) {
    struct zmk_behavior_binding binding = {
        .behavior_dev = ZMK_HOST_LIGHTING_BEHAVIOR_NAME,
        .param1 = ZMK_HOST_LIGHTING_PACK_COMMAND(command, timeout_ms),
        .param2 = value,
    };
    struct zmk_behavior_binding_event event = {
        .layer = 0,
        .position = 0,
        .timestamp = k_uptime_get(),
        .source = 0,
    };

    return zmk_split_central_invoke_behavior(0, &binding, event, true);
}

static zmk_host_lighting_ApplyResult result_for_errors(bool local_error, bool peripheral_error) {
    if (local_error && peripheral_error) {
        return zmk_host_lighting_ApplyResult_APPLY_RESULT_INTERNAL_ERROR;
    }
    if (local_error || peripheral_error) {
        return zmk_host_lighting_ApplyResult_APPLY_RESULT_PARTIAL;
    }
    return zmk_host_lighting_ApplyResult_APPLY_RESULT_OK;
}

zmk_studio_Response get_capabilities(const zmk_studio_Request *req) {
    const uint32_t pixels_per_half = zmk_rgb_underglow_host_pixel_count();
    const zmk_host_lighting_Capabilities capabilities = {
        .protocol_version = ZMK_HOST_LIGHTING_PROTOCOL_VERSION,
        .pixel_count = pixels_per_half * 2,
        .pixels_per_half = pixels_per_half,
        .max_updates_per_request = CONFIG_ZMK_HOST_LIGHTING_MAX_UPDATES,
        .max_update_hz = CONFIG_ZMK_HOST_LIGHTING_MAX_UPDATE_HZ,
        .default_timeout_ms = CONFIG_ZMK_HOST_LIGHTING_TIMEOUT_DEFAULT_MS,
        .max_timeout_ms = CONFIG_ZMK_HOST_LIGHTING_TIMEOUT_MAX_MS,
        .max_channel_value = CONFIG_ZMK_HOST_LIGHTING_MAX_CHANNEL,
        .supports_replace = true,
        .supports_split = true,
    };

    return HOST_LIGHTING_RESPONSE(get_capabilities, capabilities);
}

zmk_studio_Response set_pixels(const zmk_studio_Request *req) {
    const zmk_host_lighting_SetPixelsRequest *request =
        &req->subsystem.host_lighting.request_type.set_pixels;
    const size_t pixels_per_half = zmk_rgb_underglow_host_pixel_count();
    struct zmk_rgb_underglow_host_pixel local_updates[CONFIG_ZMK_HOST_LIGHTING_MAX_UPDATES];
    size_t local_count = 0;
    bool local_error = false;
    bool peripheral_error = false;

    for (size_t i = 0; i < request->pixels_count; i++) {
        if (request->pixels[i].index >= pixels_per_half * 2) {
            return HOST_LIGHTING_RESPONSE(
                set_pixels, zmk_host_lighting_ApplyResult_APPLY_RESULT_INVALID_PIXEL);
        }
    }

    if (request->replace) {
        local_error = zmk_rgb_underglow_host_replace(request->timeout_ms) < 0;
        peripheral_error =
            send_peripheral_command(ZMK_HOST_LIGHTING_CMD_REPLACE, 0, request->timeout_ms) < 0;
    }

    for (size_t i = 0; i < request->pixels_count; i++) {
        const zmk_host_lighting_Pixel *pixel = &request->pixels[i];
        if (pixel->index < pixels_per_half) {
            local_updates[local_count++] = (struct zmk_rgb_underglow_host_pixel){
                .index = pixel->index,
                .r = (pixel->rgb >> 16) & 0xff,
                .g = (pixel->rgb >> 8) & 0xff,
                .b = pixel->rgb & 0xff,
            };
        } else if (send_peripheral_command(pixel->index - pixels_per_half, pixel->rgb,
                                           request->timeout_ms) < 0) {
            peripheral_error = true;
        }
    }

    if (local_count > 0 &&
        zmk_rgb_underglow_host_update(local_updates, local_count, request->timeout_ms) < 0) {
        local_error = true;
    }

    return HOST_LIGHTING_RESPONSE(set_pixels, result_for_errors(local_error, peripheral_error));
}

zmk_studio_Response clear(const zmk_studio_Request *req) {
    const bool local_error = zmk_rgb_underglow_host_clear() < 0;
    const bool peripheral_error =
        send_peripheral_command(ZMK_HOST_LIGHTING_CMD_CLEAR, 0, 0) < 0;

    return HOST_LIGHTING_RESPONSE(clear, result_for_errors(local_error, peripheral_error));
}

ZMK_RPC_SUBSYSTEM_HANDLER(host_lighting, get_capabilities,
                          ZMK_STUDIO_RPC_HANDLER_UNSECURED);
ZMK_RPC_SUBSYSTEM_HANDLER(host_lighting, set_pixels, ZMK_STUDIO_RPC_HANDLER_UNSECURED);
ZMK_RPC_SUBSYSTEM_HANDLER(host_lighting, clear, ZMK_STUDIO_RPC_HANDLER_UNSECURED);
