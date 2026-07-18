#include <zephyr/kernel.h>
#include <zephyr/logging/log.h>

#include <zmk/host_lighting.h>
#include <zmk/rgb_underglow.h>
#include <zmk/split/central.h>
#include <zmk/studio/rpc.h>

LOG_MODULE_DECLARE(zmk_studio, CONFIG_ZMK_STUDIO_LOG_LEVEL);

ZMK_RPC_SUBSYSTEM(host_lighting)

#define HOST_LIGHTING_RESPONSE(type, ...) ZMK_RPC_RESPONSE(host_lighting, type, __VA_ARGS__)

static int send_peripheral_batch(const struct zmk_rgb_underglow_host_pixel *pixels,
                                 size_t pixel_count, bool replace, bool clear,
                                 uint32_t timeout_ms) {
    if (timeout_ms > UINT16_MAX) {
        return -EINVAL;
    }

    size_t offset = 0;
    do {
        const size_t count = MIN(pixel_count - offset, ZMK_SPLIT_HOST_LIGHTING_MAX_PIXELS);
        struct zmk_split_transport_host_lighting_command command = {
            .timeout_ms = timeout_ms,
            .pixel_count = count,
            .flags = (replace && offset == 0 ? ZMK_SPLIT_HOST_LIGHTING_FLAG_REPLACE : 0) |
                     (clear ? ZMK_SPLIT_HOST_LIGHTING_FLAG_CLEAR : 0),
        };

        for (size_t i = 0; i < count; i++) {
            command.pixels[i] = (struct zmk_split_transport_host_lighting_pixel){
                .index = pixels[offset + i].index,
                .r = pixels[offset + i].r,
                .g = pixels[offset + i].g,
                .b = pixels[offset + i].b,
            };
        }

        int err = zmk_split_central_update_host_lighting(0, &command);
        if (err < 0) {
            return err;
        }
        offset += count;
    } while (offset < pixel_count);

    return 0;
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
    struct zmk_rgb_underglow_host_pixel peripheral_updates[CONFIG_ZMK_HOST_LIGHTING_MAX_UPDATES];
    size_t local_count = 0;
    size_t peripheral_count = 0;
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
        } else {
            peripheral_updates[peripheral_count++] = (struct zmk_rgb_underglow_host_pixel){
                .index = pixel->index - pixels_per_half,
                .r = (pixel->rgb >> 16) & 0xff,
                .g = (pixel->rgb >> 8) & 0xff,
                .b = pixel->rgb & 0xff,
            };
        }
    }

    if (local_count > 0 &&
        zmk_rgb_underglow_host_update(local_updates, local_count, request->timeout_ms) < 0) {
        local_error = true;
    }

    if ((peripheral_count > 0 || request->replace) &&
        send_peripheral_batch(peripheral_updates, peripheral_count, request->replace, false,
                              request->timeout_ms) < 0) {
        peripheral_error = true;
    }

    return HOST_LIGHTING_RESPONSE(set_pixels, result_for_errors(local_error, peripheral_error));
}

zmk_studio_Response clear(const zmk_studio_Request *req) {
    const bool local_error = zmk_rgb_underglow_host_clear() < 0;
    const bool peripheral_error = send_peripheral_batch(NULL, 0, false, true, 0) < 0;

    return HOST_LIGHTING_RESPONSE(clear, result_for_errors(local_error, peripheral_error));
}

ZMK_RPC_SUBSYSTEM_HANDLER(host_lighting, get_capabilities,
                          ZMK_STUDIO_RPC_HANDLER_UNSECURED);
ZMK_RPC_SUBSYSTEM_HANDLER(host_lighting, set_pixels, ZMK_STUDIO_RPC_HANDLER_UNSECURED);
ZMK_RPC_SUBSYSTEM_HANDLER(host_lighting, clear, ZMK_STUDIO_RPC_HANDLER_UNSECURED);
