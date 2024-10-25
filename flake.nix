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
          version = "0.1.1";

          src = ./hetzner-private-dns-sync;
          cargoLock = {
            lockFile = ./hetzner-private-dns-sync/Cargo.lock;
          };
          buildType = "debug";

          meta = {
            description = "hetzner-private-dns-sync";
            mainProgram = "hetzner-private-dns-sync";
            maintainers = with pkgs.lib.maintainers; [ danielsidhion ];
          };
        };
      };

      nixosModule = { config, lib, pkgs, ... }:
        let cfg = config.services.hetzner-private-dns-sync;
        in {
          options.services.hetzner-private-dns-sync = {
            enable = lib.mkEnableOption "Enables the hetzner-private-dns-sync service";

            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.x86_64-linux.default;
              description = "The package to use for hetzner-private-dns-sync";
            };

            tsigKeyPath = lib.mkOption {
              type = lib.types.path;
              description = "The path to the TSIG key used to communicate with the DNS server.";
            };

            tsigKeyName = lib.mkOption {
              type = lib.types.str;
              description = "The name of the TSIG key used to communicate with the DNS server.";
            };

            serverAddress = lib.mkOption {
              type = lib.types.str;
              default = "udp://127.0.0.1:53";
              description = "The address of the DNS server to update. Must be in the format (udp|tcp)://<ip>:<port>.";
            };

            environmentFilePath = lib.mkOption {
              type = lib.types.path;
              description = "Path to a file with environment variables to further configure the software. Currently, the only required environment variable is HCLOUD_API_TOKEN.";
            };

            privateNetworkName = lib.mkOption {
              type = lib.types.str;
              description = "Name of the private network on Hetzner to use to populate the DNS server.";
            };

            zoneName = lib.mkOption {
              type = lib.types.str;
              description = "Name of the DNS zone to update. This will be the same as the internal domain you want to use.";
            };
          };

          config = lib.mkIf cfg.enable {
            systemd.services.hetzner-private-dns-sync = {
              description = "Runs hetzner-private-dns-sync once to sync server IPs and hostnames into a DNS server.";

              serviceConfig = {
                Type = "oneshot";
                EnvironmentFile = cfg.environmentFilePath;
                ExecStart = "${lib.getExe cfg.package} --tsig-key-path ${cfg.tsigKeyPath} --tsig-key-name ${cfg.tsigKeyName} --server-address ${cfg.serverAddress} --private-network-name ${cfg.privateNetworkName} --zone-name ${cfg.zoneName}";
                DynamicUser = true;
                User = "hetzner-private-dns-sync";
                StateDirectory = "hetzner-private-dns-sync";
                StateDirectoryMode = "0750";
              };
            };

            systemd.timers.hetzner-private-dns-sync = {
              description = "Timer for hetzner-private-dns-sync.";

              wantedBy = [ "timers.target" ];

              timerConfig = {
                AccuracySec = "1us";
                OnStartupSec = "10s";
                OnUnitInactiveSec = "10s";
              };
            };
          };
        };

    };
}
