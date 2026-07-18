{
  description = "Ivan's Glove80 ZMK Studio configuration";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-22.05";
    flake-utils.url = "github:numtide/flake-utils";
    zmk = {
      url = "github:moergo-sc/zmk";
      flake = false;
    };
  };

  outputs = { self, nixpkgs, flake-utils, zmk }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
        zmkPkgs = import (zmk + "/nix/pinned-nixpkgs.nix") { inherit system; };
        patchedZmk = zmkPkgs.applyPatches {
          name = "moergo-zmk-studio";
          src = zmk;
          patches = [
            ./nix/moergo-zmk-studio-modules.patch
            ./nix/moergo-zmk-host-lighting.patch
            ./nix/moergo-zmk-status-pixel.patch
            ./nix/moergo-zmk-power-led.patch
          ];
        };
        firmware = import patchedZmk { pkgs = zmkPkgs; };
      in
      {
        packages.firmware = import ./config {
          inherit pkgs firmware;
        };
        packages.default = self.packages.${system}.firmware;

        apps.generate-keymap = {
          type = "app";
          program = "${pkgs.writeShellScript "generate-glove80-keymap" ''
            exec ${pkgs.nodejs}/bin/node "$PWD/scripts/generate-keymap.mjs"
          ''}";
        };

        devShells.default = pkgs.mkShell {
          packages = [
            pkgs.nodejs
            pkgs.nixpkgs-fmt
          ];
        };
      });
}
