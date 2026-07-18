{ pkgs ? import <nixpkgs> {}
, firmware ? import ../src { inherit pkgs; }
}:

let
  config = ./.;
  hostLightingModule = ../host-lighting;
  maintenanceModule = ../maintenance;
  studioMessagesOverlay = ../protocol;

  glove80_left = firmware.zmk.override {
    board = "glove80_lh";
    keymap = "${config}/glove80.keymap";
    kconfig = "${config}/glove80.conf";
    snippets = [ "studio-rpc-usb-uart" ];
    extraModules = [ hostLightingModule maintenanceModule ];
    inherit studioMessagesOverlay;
  };

  glove80_right = firmware.zmk.override {
    board = "glove80_rh";
    keymap = "${config}/glove80.keymap";
    kconfig = "${config}/glove80.conf";
    extraModules = [ hostLightingModule maintenanceModule ];
    inherit studioMessagesOverlay;
  };

  glove80_left_settings_reset = firmware.zmk.override {
    board = "glove80_lh";
    shield = "settings_reset";
  };

  glove80_right_settings_reset = firmware.zmk.override {
    board = "glove80_rh";
    shield = "settings_reset";
  };
in
pkgs.runCommandNoCC "glove80-firmware" {} ''
  mkdir -p "$out"
  cp ${glove80_left}/zmk.uf2 "$out/glove80-left.uf2"
  cp ${glove80_right}/zmk.uf2 "$out/glove80-right.uf2"
  cp ${glove80_left_settings_reset}/zmk.uf2 "$out/glove80-left-settings-reset.uf2"
  cp ${glove80_right_settings_reset}/zmk.uf2 "$out/glove80-right-settings-reset.uf2"
  cat ${glove80_left}/zmk.uf2 ${glove80_right}/zmk.uf2 > "$out/glove80.uf2"
''
