{
  description = "willhaben-renewer";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs { inherit system; };
      in
      {
        # Build the crate first, then wrap it with chromedriver on PATH as default.
        packages = let
          crate = pkgs.rustPlatform.buildRustPackage {
            pname = "willhaben-renewer";
            version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version;
            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;
            buildInputs = with pkgs; [ openssl pkg-config ];
            nativeBuildInputs = with pkgs; [ pkg-config ];
          };
          wrapper = pkgs.writeShellScriptBin "willhaben-renewer" ''
            #!/usr/bin/env bash
            set -euo pipefail
            export PATH=${pkgs.chromedriver}/bin:$PATH
            exec ${crate}/bin/willhaben-renewer "$@"
          '';
        in {
          crate = crate;       # raw binary (no chromedriver)
          default = wrapper;   # wrapped with chromedriver in PATH
        };
      }
    );
}
