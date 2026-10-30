# ZayMusic

> [!WARNING]
> **This is a personal project, not for anyone else to use.** It is published only so
> its owner can use it conveniently. It is built for one person's own home
> server, devices and Tailscale network. It is not supported, documented or secured for
> other deployments, and issues, pull requests and feature requests will not be
> accepted. No license is granted: all rights are reserved. Please do not run, fork,
> redistribute or build on it.

Self-hosted music app: YouTube Music’s library depth, a Spotify-quality interface, zero subscriptions. Runs on a home server, reachable over Tailscale, played through native iOS and Linux clients.

See [PLAN.md](./PLAN.md) for the full architecture and constraints.

## Stack

- **crates/zay-gateway** — Rust (axum) API gateway on `127.0.0.1:3847`. Proxies OpenSubsonic (`/rest/*`) and ZayFetch (`/api/fetch/*`), injecting Navidrome token auth and the ZayFetch token server-side; streams media (Range/HEAD) without buffering. Also `/healthz` and `/api/native/config`.
- **crates/zay-core** — shared native Rust/UniFFI queue, discovery, catalog and offline storage.
- **apps/linux** — Relm4/GTK4 desktop client with GStreamer and MPRIS.
- **apps/ios** — SwiftUI client with AVQueuePlayer; unsigned IPA builds on GitHub macOS runners.
- **services/zayfetch** — Python FastAPI acquisition service (yt-dlp, tagging, ReplayGain, job queue).
- **navidrome** — library scan, tags, cover art, streaming, playlists, stars (official image).

## Local development (devbox)

```bash
# one-time
cp dev.env.example dev.env
# fill secrets (note: gen-secrets prints PROD paths — keep the loopback URLs from dev.env)
./scripts/gen-secrets.sh | sed -e 's#^MUSIC_DIR=.*#MUSIC_DIR=./dev-music#' \
  -e 's#^NAVIDROME_DATA_DIR=.*#NAVIDROME_DATA_DIR=./dev-navidrome#' \
  -e 's#^ZAYFETCH_DATA_DIR=.*#ZAYFETCH_DATA_DIR=./dev-zayfetch#' \
  >> dev.env
# or just edit secrets by hand in dev.env

# containers (navidrome, zayfetch, browser, gateway on loopback :3847)
./scripts/dev-up.sh
# or step by step:
docker compose -f docker-compose.yml -f docker-compose.dev.yml --env-file dev.env up -d --build
docker compose -f docker-compose.yml -f docker-compose.dev.yml --env-file dev.env \
  run --rm zayfetch python -m zayfetch.bootstrap_admin
```

Point the native clients at `http://127.0.0.1:3847` (or the Tailscale URL below).
Developing the gateway alone: `cargo run -p zay-gateway` with the same environment
keys (`NAVIDROME_URL`, `ZAYFETCH_URL`, …; `HOST`/`PORT` default to `127.0.0.1:3000`).

From a worktree, keep dev data and `dev.env` anchored at the main checkout and
override build contexts with an ignored file such as `build/native-dev-source.yml`:

```bash
docker compose --project-directory /home/isaiah/Projects/ZayMusic \
  --env-file /home/isaiah/Projects/ZayMusic/dev.env \
  -f docker-compose.yml -f docker-compose.dev.yml -f build/native-dev-source.yml \
  up -d --build --no-deps zaymusic
```

Over Tailscale:

```bash
# once per boot if serve was cleared:
sudo tailscale set --operator="$(id -un)"
tailscale serve --bg --https=8445 http://127.0.0.1:3847
# → https://devbox.tailfc5dc.ts.net:8445
```

Dev talks to loopback URLs (`http://127.0.0.1:4533`, `http://127.0.0.1:8081`). Production uses compose hostnames. This is why `docker-compose.dev.yml` exists — never merge it into the base file and never ship it.

## Tests

```bash
./scripts/native-local-checks.sh            # all: native + gateway + services
./scripts/native-local-checks.sh gateway    # gateway unit/mock tests, clippy, image + fixture smoke
cargo test --locked -p zay-gateway
cargo test --locked -p zay-core
```

`./scripts/native-gateway-fixture-smoke.sh` runs the gateway (binary, or
`NATIVE_GATEWAY_IMAGE=<tag>`) in front of the loopback fixture with fake credentials.

## Ship (pull-based deploy)

devbox **never** SSHes to `fedora-server`. You produce a bundle; the host pulls it.

```bash
./scripts/mkbundle.sh
# tell the owner:
#   deploy ZayMusic from ~/Projects/ZayMusic/deploy-bundle/

# after the manager session confirms:
./scripts/wait-healthy.sh "$(python3 -c "import json;print(json.load(open('deploy-bundle/app-manifest.json'))['sha'])")"
SMOKE_BASE=https://fedora-server.tailfc5dc.ts.net ./scripts/smoke.sh
```

Host runbook ships as `deploy-bundle/README-DEPLOY.md` (source of truth: `services/deploy/README-DEPLOY.md.tmpl`).

## Environment

| File | Where | Purpose |
|---|---|---|
| `.env.example` | repo + bundle | fake values, documented |
| `dev.env` | devbox only (gitignored) | local secrets + loopback URLs |
| `.env` | host only (mode 600) | real secrets — never committed |

Generated with `./scripts/gen-secrets.sh`.

## Hard rules (abridged — see PLAN §14)

- No `--audio-format mp3` / re-encodes — keep native Opus/AAC.
- No title-keyword filters on acquisition — rank and badge only.
- Tailscale-only: only `127.0.0.1:${APP_PORT}` published; never host port 8080. No app-level login; the gateway refuses cross-site browser requests.
- Service credentials (Navidrome password, ZayFetch token) live only in the gateway environment; clients never send or receive them.
- No SSH from devbox → host; no git-hook deploys.

## Native clients

The SvelteKit web client was removed; the native apps are the only clients. Its
last source (behaviour reference and tests) is commit `93f43ec:apps/web/` in the
owner's private pre-squash archive; this public history starts after its removal. See [the parity checklist](docs/native/PARITY.md),
[device testing and delivery](docs/native/TESTING.md) and
[implementation decisions](docs/native/DECISIONS.md).

On devbox, `cargo test -p zay-core` runs shared behavior/fixture tests and
`./scripts/native-check-swift.sh` checks the Swift bridge without a Mac.
`./scripts/native-linux-handoff.sh` verifies native short-track advancement with
temporary silence; `./scripts/native-linux-smoke.sh` exercises the development
stack silently under Xvfb. Install a requested desktop build with
`./scripts/native-install-linux.sh --mypc`. There is no automatic update channel.

Native, gateway and service dependency/toolchain versions and update evidence: [docs/native/DEPENDENCIES.md](docs/native/DEPENDENCIES.md).
