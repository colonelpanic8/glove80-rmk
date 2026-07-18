#include <zephyr/kernel.h>
#include <zephyr/sys/reboot.h>

#include <dt-bindings/zmk/reset.h>
#include <zmk/behavior.h>
#include <zmk/endpoints.h>
#include <zmk/split/central.h>
#include <zmk/studio/rpc.h>

ZMK_RPC_SUBSYSTEM(maintenance)

#define MAINTENANCE_RESPONSE(type, ...) ZMK_RPC_RESPONSE(maintenance, type, __VA_ARGS__)

static void enter_bootloader_work_handler(struct k_work *work) { sys_reboot(RST_UF2); }

K_WORK_DELAYABLE_DEFINE(enter_bootloader_work, enter_bootloader_work_handler);

zmk_studio_Response enter_bootloader(const zmk_studio_Request *req) {
    if (zmk_endpoints_selected().transport != ZMK_TRANSPORT_USB) {
        return MAINTENANCE_RESPONSE(enter_bootloader, false);
    }

    if (req->subsystem.maintenance.request_type.enter_bootloader ==
        zmk_maintenance_BootloaderTarget_BOOTLOADER_TARGET_RIGHT) {
        struct zmk_behavior_binding binding = {
            .behavior_dev = "bootload",
        };
        struct zmk_behavior_binding_event event = {
            .layer = 0,
            .position = 0,
            .timestamp = k_uptime_get(),
            .source = 0,
        };

        return MAINTENANCE_RESPONSE(
            enter_bootloader,
            zmk_split_central_invoke_behavior(0, &binding, event, true) >= 0);
    }

    k_work_reschedule(&enter_bootloader_work,
                      K_MSEC(CONFIG_ZMK_MAINTENANCE_BOOTLOADER_DELAY_MS));
    return MAINTENANCE_RESPONSE(enter_bootloader, true);
}

ZMK_RPC_SUBSYSTEM_HANDLER(maintenance, enter_bootloader, ZMK_STUDIO_RPC_HANDLER_UNSECURED);
