{
  description = "hetzner-private-dns-sync";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    nil = {
      url = "github:oxalica/nil";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, nil, rust-overlay, ... }:
    let
      pkgs = import nixpkgs {
        overlays = [ (import rust-overlay) ];
        system = "x86_64-linux";
      };
    in
    {
      devShells.x86_64-linux = {
        default = pkgs.mkShell {
          packages = with pkgs; [
            (rust-bin.stable.latest.default.override
              {
                extensions = [ "rust-src" "rustfmt" "rust-analyzer" "clippy" ];
              })

            # Both of these used with VSCode.
            nixpkgs-fmt
            nil.packages.${system}.default
          ];

          env = {
            RUST_BACKTRACE = "full";
          };
        };
      };

      packages.x86_64-linux = {
        default = pkgs.rustPlatform.buildRustPackage {
          pname = "hetzner-private-dns-sync";
          version = "0.1.0";

          src = ./hetzner-private-dns-sync;
          cargoLock = {
            lockFile = ./hetzner-private-dns-sync/Cargo.lock;
            outputHashes = {
              "hcloud-0.20.0" = "sha256-d47CTCPd+rNczyPp3aJ7NFuqN2X1SWlsVRoVVNX5Aqs=";
            };
          };
          buildType = "debug";

          meta = {
            description = "hetzner-private-dns-sync";
            mainProgram = "hetzner-private-dns-sync";
            maintainers = with pkgs.lib.maintainers; [ danielsidhion ];
          };
        };
      };
    };
}
