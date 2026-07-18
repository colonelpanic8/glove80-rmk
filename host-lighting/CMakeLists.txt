zephyr_include_directories(include)

target_sources_ifdef(CONFIG_ZMK_HOST_LIGHTING_RPC app PRIVATE
    src/host_lighting_subsystem.c)
