# mjolnix

![mjolnix logo](assets/mjolnix.png) (source: [src/assets/mjolnix.png](src/assets/mjolnix.png))

Git hosting over SSH with automatic Nix flake builds on push.

When you connect over SSH (`ssh my-host` / `mjolnix` TUI), the logo is shown in terminals that support the [Kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/) (Kitty, Ghostty, etc.). Set `MJOLNIX_NO_LOGO=1` to disable.

## Quick start (local)

PostgreSQL:

```bash
docker compose up -d
export MJOLNIX_DATABASE_URL=postgres://mjolnix:mjolnix@127.0.0.1:5432/mjolnix
```

See `.env.example` for a full local env template.

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

## Per-repo Nix stores and binary cache

Each repository has its own Nix store under `$MJOLNIX_DATA_DIR/stores/<repo-id>/`. Builds run with `nix build --store …` against that store only (not the host `/nix/store`).

When `MJOLNIX_CACHE_ENABLE` is set (default: on), `mjolnixd` also serves an HTTP binary cache per repo at:

`http://<MJOLNIX_CACHE_HOST>:<port>/r/<namespace>/<name>`

After a successful build, the SSH TUI shows the substituter URL and `trusted-public-keys` for that repo. Signing key material is stored at `$MJOLNIX_DATA_DIR/cache-secret-key` by default.

## Environment

| Variable | Purpose |
|----------|---------|
| `MJOLNIX_DATABASE_URL` | PostgreSQL connection URL (required) |
| `MJOLNIX_DATA_DIR` | Bare repos, workdirs, logs, socket (default: `~/.local/share/mjolnix`) |
| `MJOLNIX_KEY_FINGERPRINT` | SSH key identity for git/TUI |
| `MJOLNIX_STORES_DIR` | Per-repo Nix store roots (default: `$MJOLNIX_DATA_DIR/stores`) |
| `MJOLNIX_CACHE_ENABLE` | Enable built-in HTTP cache in `mjolnixd` (default: `true`) |
| `MJOLNIX_CACHE_BIND` / `MJOLNIX_CACHE_HOST` / `MJOLNIX_CACHE_PORT` | Cache listener and URL host used in substituter paths |
| `MJOLNIX_CACHE_SIGN_KEY_PATH` / `MJOLNIX_CACHE_KEY_NAME` | Binary cache signing key |
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
            binaryCache.enable = true;
            # Bundled PostgreSQL (peer auth as user `git`) is enabled by default.
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
- **PostgreSQL** — users, SSH keys, repos, builds (`closure_paths` JSONB on success)
- Push with `flake.nix` → `post-receive` queues a build → daemon runs `nix build`
