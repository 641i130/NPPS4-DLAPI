{
  description = "NPPS4-DLAPI Rust development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs { inherit system overlays; };
        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" "clippy" "rustfmt" ];
        };
        honkypy = pkgs.python3Packages.buildPythonPackage {
          pname = "honkypy";
          version = "0.2.0";
          format = "wheel";
          src = pkgs.fetchurl {
            url = "https://github.com/DarkEnergyProcessor/honky-py/releases/download/0.2.0/honkypy-0.2.0-py3-none-any.whl";
            sha256 = "02dijp5j0fs0js1rf0qaicq4zc0npx0wrmyvib95vvqvsbkzmwgy";
          };
        };
        pythonWithHonkypy = pkgs.python3.withPackages (_: [ honkypy ]);
      in {
        devShells.default = pkgs.mkShell {
          nativeBuildInputs = [
            rustToolchain
            pkgs.cargo-watch
            pkgs.pkg-config
            pythonWithHonkypy
          ];

          buildInputs = [
            pkgs.openssl
          ];

          env = {
            RUST_LOG = "info";
          };

          shellHook = ''
            echo "NPPS4-DLAPI Rust development environment"
            echo "Rust: $(rustc --version)"
            echo ""
            echo "Commands:"
            echo "  cargo build          - Build"
            echo "  cargo run            - Run server"
            echo "  cargo watch -x run   - Auto-restart on changes"
            echo ""
            echo "Environment variables:"
            echo "  N4DLAPI_CONFIG_FILE  - Path to config.toml (default: config.toml)"
            echo "  N4DLAPI_ARCHIVE_ROOT - Override archive root directory"
            echo "  N4DLAPI_LISTEN       - Listen address (default: 127.0.0.1:8000)"
            echo "  RUST_LOG             - Log level (default: info)"
          '';
        };

        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "n4dlapi";
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          nativeBuildInputs = [ pkgs.pkg-config ];
          buildInputs = [ pkgs.openssl ];
        };
      }
    );
}
