# mjolnix

![mjolnix logo](assets/mjolnix.png) (source: [src/assets/mjolnix.png](src/assets/mjolnix.png))

Git hosting over SSH with automatic Nix flake builds on push.

When you connect over SSH (`ssh my-host` / `mjolnix` TUI), the logo is shown in terminals that support the [Kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/) (Kitty, Ghostty, etc.). Set `MJOLNIX_NO_LOGO=1` to disable.

## Quick start (local)

```bash
nix develop
cargo build

# Interactive TUI (create repos)
mjolnix

# Build daemon (required for Nix builds on push)
run-mjolnixd

# Git over fake SSH
git-with-mjolnix clone localhost:public/demo.git
git-with-mjolnix -C my-repo push origin main
```

Install hooks on repos created before hooks existed:

```bash
mjolnix install-hooks
```

## Nix binary cache (Harmonia)

Successful builds populate the host Nix store. Serve them with [Harmonia](https://github.com/nix-community/harmonia):

```bash
# In dev shell (see harmonia-dev.toml)
nix run .#harmonia -- -c harmonia-dev.toml
```

Client configuration:

```ini
extra-substituters = http://127.0.0.1:5000
trusted-public-keys = <your-harmonia-public-key>
```

Set `MJOLNIX_SUBSTITUTER_URL` so the SSH TUI prints copy hints after successful builds.

## Environment

| Variable | Purpose |
|----------|---------|
| `MJOLNIX_DATA_DIR` | Database, bare repos, workdirs, socket (default: `~/.local/share/mjolnix`) |
| `MJOLNIX_KEY_FINGERPRINT` | SSH key identity for git/TUI |
| `MJOLNIX_SUBSTITUTER_URL` | Binary cache URL shown in TUI |
| `MJOLNIX_MAX_PARALLEL_BUILDS` | Daemon concurrency (default: 2) |
| `MJOLNIX_BUILD_TIMEOUT_SECS` | Per-build timeout (default: 3600) |

## NixOS module

```nix
{
  inputs.mjolnix.url = "github:YOUR_ORG/mjolnix";

  outputs = { inputs, ... }: {
    nixosConfigurations.my-server = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        inputs.mjolnix.nixosModules.default
        {
          nixpkgs.overlays = [ inputs.mjolnix.overlays.default ];
          services.mjolnix = {
            enable = true;
            host = "git.example.com";
            authorizedKeys = [
              "ssh-ed25519 AAAA... you@laptop"
            ];
            binaryCache.enable = true; # Harmonia on port 5000
          };
        }
      ];
    };
  };
}
```

Verify with `nix flake check` (runs the NixOS test).

## Architecture

- `mjolnix` — SSH entry (git wrapper + TUI + hooks)
- `mjolnixd` — build worker (Unix socket at `$MJOLNIX_DATA_DIR/mjolnixd.sock`)
- Push with `flake.nix` → `post-receive` queues a build → daemon runs `nix build`
