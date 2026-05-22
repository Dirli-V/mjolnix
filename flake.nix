{
  description = "Build mjolnix";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

    crane.url = "github:ipetkov/crane";
  };

  outputs =
    {
      self,
      nixpkgs,
      crane,
      ...
    }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];

      forAllSystems =
        f:
        nixpkgs.lib.genAttrs systems (
          system:
          f {
            inherit system;
            pkgs = nixpkgs.legacyPackages.${system};
          }
        );

      perSystem =
        {
          pkgs,
          system,
        }:
        let
          inherit (pkgs) lib;

          craneLib = crane.mkLib pkgs;
          # cleanCargoSource omits PNG assets required by include_bytes!
          src = pkgs.runCommandLocal "mjolnix-source" { } ''
            mkdir -p $out
            cp -r ${craneLib.cleanCargoSource ./.}/* $out/
            chmod -R u+w $out
            install -D ${./src/assets/mjolnix.png} $out/src/assets/mjolnix.png
          '';

          commonArgs = {
            inherit src;
            strictDeps = true;
            buildInputs = [ ];
          };

          cargoArtifacts = craneLib.buildDepsOnly commonArgs;

          mjolnix = craneLib.buildPackage (
            commonArgs
            // {
              inherit cargoArtifacts;
            }
          );
        in
        {
          checks = {
            inherit mjolnix;

            mjolnix-clippy = craneLib.cargoClippy (
              commonArgs
              // {
                inherit cargoArtifacts;
                cargoClippyExtraArgs = "--all-targets -- --deny warnings";
              }
            );

            mjolnix-doc = craneLib.cargoDoc (
              commonArgs
              // {
                inherit cargoArtifacts;
              }
            );

            mjolnix-fmt = craneLib.cargoFmt {
              inherit src;
            };

            mjolnix-toml-fmt = craneLib.taploFmt {
              src = pkgs.lib.sources.sourceFilesBySuffices src [ ".toml" ];
            };

            nixosTest = import ./nixos-tests/mjolnix.nix {
              inherit pkgs lib;
              package = mjolnix;
            };
          };

          packages = {
            default = mjolnix;
            inherit mjolnix;
            mjolnixd = mjolnix;
          };

          apps = {
            default = {
              type = "app";
              program = "${mjolnix}/bin/mjolnix";
              meta = (mjolnix.meta or { }) // {
                description = "mjolnix";
                mainProgram = "mjolnix";
              };
            };
            mjolnixd = {
              type = "app";
              program = "${mjolnix}/bin/mjolnixd";
              meta = (mjolnix.meta or { }) // {
                description = "mjolnix build daemon";
                mainProgram = "mjolnixd";
              };
            };
            harmonia = {
              type = "app";
              program = "${pkgs.harmonia}/bin/harmonia";
              meta = {
                description = "Nix binary cache for mjolnix build outputs";
              };
            };
          };

          devShells = {
            default = craneLib.devShell {
              checks = self.checks.${system};
              packages = [
                pkgs.git
                pkgs.nix
                pkgs.harmonia
                pkgs.rust-analyzer
                pkgs.rustfmt
              ];
              shellHook = ''
                root="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
                export PATH="$root/scripts:''${PATH}"
                export MJOLNIX_DATA_DIR="''${MJOLNIX_DATA_DIR:-''${XDG_DATA_HOME:-$HOME/.local/share}/mjolnix}"
                export MJOLNIX_KEY_FINGERPRINT="''${MJOLNIX_KEY_FINGERPRINT:-dev:local}"
                export MJOLNIX_BIN="$root/target/debug/mjolnix"
                export MJOLNIX_SUBSTITUTER_URL="''${MJOLNIX_SUBSTITUTER_URL:-http://127.0.0.1:5000}"
                echo "mjolnix dev: data=$MJOLNIX_DATA_DIR substituter=$MJOLNIX_SUBSTITUTER_URL"
                echo "  run-mjolnixd   start build daemon"
                echo "  nix run .#harmonia -- -c $PWD/harmonia-dev.toml   binary cache (optional)"
              '';
            };
          };
        };

      systemOutputs = forAllSystems perSystem;
    in
    {
      overlays.default = final: prev: {
        mjolnix = self.packages.${final.system}.default;
      };

      nixosModules.default = ./nix/modules/mjolnix.nix;
      nixosModules.mjolnix = self.nixosModules.default;

      checks = nixpkgs.lib.mapAttrs (_: o: o.checks) systemOutputs;
      packages = nixpkgs.lib.mapAttrs (_: o: o.packages) systemOutputs;
      apps = nixpkgs.lib.mapAttrs (_: o: o.apps) systemOutputs;
      devShells = nixpkgs.lib.mapAttrs (_: o: o.devShells) systemOutputs;
    };
}