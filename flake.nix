{
  description = "Build mjolnix";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

    crane.url = "github:ipetkov/crane";
  };

  outputs = {
    self,
    nixpkgs,
    crane,
    ...
  }: let
    systems = [
      "x86_64-linux"
      "aarch64-linux"
    ];

    forAllSystems = f:
      nixpkgs.lib.genAttrs systems (
        system:
          f {
            inherit system;
            pkgs = nixpkgs.legacyPackages.${system};
          }
      );

    perSystem = {
      pkgs,
      system,
    }: let
      inherit (pkgs) lib;

      craneLib = crane.mkLib pkgs;

      src = lib.fileset.toSource {
        root = ./.;
        fileset = lib.fileset.unions [
          ./Cargo.toml
          ./Cargo.lock
          ./migrations
          (craneLib.fileset.commonCargoSources ./crates/mjolnix-shared)
          (craneLib.fileset.commonCargoSources ./crates/mjolnix-worker)
          (craneLib.fileset.commonCargoSources ./crates/mjolnix-cache)
          (craneLib.fileset.commonCargoSources ./crates/mjolnix-frontend)
        ];
      };

      commonArgs = {
        inherit src;
        strictDeps = true;
        buildInputs = [];
        cargoExtraArgs = "--workspace --locked";
      };

      cargoArtifacts = craneLib.buildDepsOnly commonArgs;

      mjolnix = craneLib.buildPackage (
        commonArgs
        // {
          inherit cargoArtifacts;
          postInstall = ''
            ln -sf mjolnix-frontend $out/bin/mjolnix
          '';
        }
      );

      start_db = pkgs.writeShellScriptBin "start_db" ''
        set -euo pipefail
        exec docker compose up -d
      '';

      frontend = pkgs.writeShellScriptBin "frontend" ''
        set -euo pipefail
        exec cargo run -p mjolnix-frontend
      '';

      worker = pkgs.writeShellScriptBin "worker" ''
        set -euo pipefail
        exec cargo run -p mjolnix-worker
      '';

      cache = pkgs.writeShellScriptBin "cache" ''
        set -euo pipefail
        exec cargo run -p mjolnix-cache
      '';
    in {
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
          src = pkgs.lib.sources.sourceFilesBySuffices src [".toml"];
        };

        nixosTest = import ./nixos-tests/mjolnix.nix {
          inherit pkgs lib;
          package = mjolnix;
        };
      };

      packages = {
        default = mjolnix;
        inherit mjolnix;
      };

      apps = {
        default = {
          type = "app";
          program = "${mjolnix}/bin/mjolnix-frontend";
          meta =
            (mjolnix.meta or {})
            // {
              description = "mjolnix SSH frontend";
              mainProgram = "mjolnix-frontend";
            };
        };
        frontend = {
          type = "app";
          program = "${mjolnix}/bin/mjolnix-frontend";
        };
        worker = {
          type = "app";
          program = "${mjolnix}/bin/mjolnix-worker";
        };
        cache = {
          type = "app";
          program = "${mjolnix}/bin/mjolnix-cache";
        };
      };

      devShells = {
        default = craneLib.devShell {
          checks = self.checks.${system};
          packages = [
            pkgs.git
            pkgs.nix
            pkgs.xz
            pkgs.rust-analyzer
            pkgs.rustfmt
            start_db
            frontend
            worker
            cache
          ];
          shellHook = ''
            export MJOLNIX_DATABASE_URL=postgres://mjolnix:mjolnix@127.0.0.1:5432/mjolnix
            echo "Run 'start_db' to start the database"
            echo "Run 'frontend', 'worker', or 'cache'"
          '';
        };
      };
    };

    systemOutputs = forAllSystems perSystem;
  in {
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
