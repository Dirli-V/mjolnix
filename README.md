# mjolnix

![mjolnix logo](assets/mjolnix.png)

Git hosting over SSH with automatic Nix flake builds on push.

The codebase is a Cargo workspace with separate binaries that share PostgreSQL state via `mjolnix-shared`. Run the frontend, worker, and cache as independent processes (three terminals in local dev).

## Crates

| Crate / binary | Role |
|----------------|------|
| `mjolnix-frontend` | SSH git wrapper, interactive TUI, `post-receive` hooks |
| `mjolnix-worker` | Polls queued builds in Postgres and runs `nix build` |
| `mjolnix-cache` | HTTP binary cache (`narinfo` / `nar`) per repository |
| `mjolnix-shared` | Config, database access, store paths, cache signing (library) |

Coordination between components is through PostgreSQL only (for example hooks insert `builds` rows with status `queued`; the worker picks them up). There is no Unix socket between services.

## Quick start (local)

PostgreSQL:

```bash
docker compose up -d
export MJOLNIX_DATABASE_URL=postgres://mjolnix:mjolnix@127.0.0.1:5432/mjolnix
```

Build and run:

```bash
nix develop
cargo build

# Terminal 1 — interactive TUI (create repos)
cargo run -p mjolnix-frontend

# Terminal 2 — Nix builds on push
cargo run -p mjolnix-worker

# Terminal 3 — binary cache (substituters)
cargo run -p mjolnix-cache
```

Git over the helper SSH transport (points `MJOLNIX_BIN` at the frontend binary):

```bash
export MJOLNIX_BIN="$PWD/target/debug/mjolnix-frontend"
git-with-mjolnix clone localhost:public/demo.git
git-with-mjolnix -C demo push origin main
```

Inside `nix develop`, `MJOLNIX_DATA_DIR`, `MJOLNIX_DATABASE_URL`, and `MJOLNIX_KEY_FINGERPRINT=dev:local` are set in the shell hook. Set `MJOLNIX_BIN` to your built `mjolnix-frontend` if you use `git-with-mjolnix` outside that shell.

## Per-repo Nix stores and binary cache

Each repository has its own Nix store under `$MJOLNIX_DATA_DIR/stores/<repo-id>/`. The worker runs `nix build --store …` against that store only (not the host `/nix/store`).

`mjolnix-cache` serves an HTTP binary cache per repo at:

`http://<MJOLNIX_CACHE_HOST>:<MJOLNIX_CACHE_PORT>/r/<namespace>/<name>`

After a successful build, the TUI shows the substituter URL and `trusted-public-keys` for that repo. Signing key material defaults to `$MJOLNIX_DATA_DIR/cache-secret-key`.

## Environment

| Variable | Purpose |
|----------|---------|
| `MJOLNIX_DATABASE_URL` | PostgreSQL connection URL (required) |
| `MJOLNIX_DATA_DIR` | Bare repos, workdirs, logs, signing key (default: `~/.local/share/mjolnix`) |
| `MJOLNIX_HOST` | Hostname in clone URLs (default: `localhost`) |
| `MJOLNIX_KEY_FINGERPRINT` | SSH key identity for git/TUI |
| `MJOLNIX_USER_ID` | Optional fixed user id (dev/TUI) |
| `MJOLNIX_FRONTEND_BIN` | Path used in `post-receive` hooks (default: current exe) |
| `MJOLNIX_BIN` | Used by `scripts/git-with-mjolnix` — set to `mjolnix-frontend` |
| `MJOLNIX_STORES_DIR` | Per-repo Nix store roots (default: `$MJOLNIX_DATA_DIR/stores`) |
| `MJOLNIX_CACHE_BIND` / `MJOLNIX_CACHE_HOST` / `MJOLNIX_CACHE_PORT` | Cache listener and URL host in substituter paths |
| `MJOLNIX_CACHE_SIGN_KEY_PATH` / `MJOLNIX_CACHE_KEY_NAME` | Binary cache signing key |
| `MJOLNIX_MAX_PARALLEL_BUILDS` | Worker concurrency (default: 2) |
| `MJOLNIX_BUILD_TIMEOUT_SECS` | Per-build timeout (default: 3600) |

## NixOS module

The flake packages all three binaries. `services.mjolnix` enables `mjolnix-worker` and (by default) `mjolnix-cache`; SSH login runs `mjolnix-frontend`. A `mjolnix` symlink points at the frontend for compatibility.

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
          };
        }
      ];
    };
  };
}
```

Verify with `nix flake check` (runs the NixOS test).

## Architecture

```mermaid
flowchart LR
  subgraph clients [Clients]
    SSH[SSH / git]
    TUI[Terminal TUI]
  end
  subgraph frontend [mjolnix-frontend]
    Git[git-receive-pack / upload-pack]
    Hook[post-receive hook]
  end
  DB[(PostgreSQL)]
  subgraph worker [mjolnix-worker]
    Build[nix build]
  end
  subgraph cache [mjolnix-cache]
    HTTP[HTTP narinfo / nar]
  end
  SSH --> Git
  TUI --> frontend
  Git --> Hook
  Hook -->|INSERT build queued| DB
  worker -->|poll queued| DB
  worker --> Build
  Build -->|UPDATE success / failed| DB
  HTTP -->|repo_stores metadata| DB
  clients -->|substituter fetch| HTTP
```

- **PostgreSQL** — users, SSH keys, repos, `repo_stores`, builds (`closure_paths` JSONB on success)
- Push with `flake.nix` → `post-receive` queues a build row → **worker** runs `nix build`
- **Cache** reads store paths from the database and serves artifacts built into each repo store
