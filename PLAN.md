> Native migration scope: the web client is retired at the owner’s request.
> Web-specific implementation references below are historical; the behavior
> reference is `93f43ec:apps/web/...` (private pre-squash archive). Current clients are Linux and iPhone;
> their API gateway is `crates/zay-gateway`. Authentication ownership remains as
> documented here. See `docs/native/DECISIONS.md` for current decisions.

# ZayMusic — Implementation Plan

> **For the implementing agent:** This document is fully self-contained. It is written for an agent with **no prior context**. Do not ask the user to clarify anything covered here. Implement it. If you hit a genuine contradiction or a missing fact that blocks progress, note it in `NOTES.md` and pick the most reasonable option so work continues.
>
> **Owner:** one person (Isaiah). This is a personal, private, never-released app. No multi-tenancy, no accessibility audit, no app-store compliance, no billing, no analytics, no GDPR. Prefer simple over correct.
>
> **Product in one line:** A self-hosted music app that replaces the *interface* of YouTube Music while keeping (and growing) its *library depth* — playable from iPhone (PWA) and desktop (browser), offline-capable, with a pipeline that pulls new music in from YouTube.

---

## Table of contents

1. [Vision & scope](#1-vision--scope)
2. [Environment — real facts about the machines](#2-environment--real-facts-about-the-machines)
3. [Must-know technical constraints](#3-must-know-technical-constraints)
4. [Architecture](#4-architecture)
5. [Repository layout](#5-repository-layout)
6. [Server components](#6-server-components)
7. [Client architecture](#7-client-architecture)
8. [Data models & API contracts](#8-data-models--api-contracts)
9. [Acquisition pipeline (detailed)](#9-acquisition-pipeline-detailed)
10. [Offline subsystem](#10-offline-subsystem)
11. [Deployment — pull-based bundle](#11-deployment--pull-based-bundle)
12. [Milestones & acceptance criteria](#12-milestones--acceptance-criteria)
13. [Out of scope for v1](#13-out-of-scope-for-v1)
14. [Pitfalls — read before writing code](#14-pitfalls--read-before-writing-code)
15. [Verification](#15-verification)
16. [Post-v1 backlog (do not implement now)](#16-post-v1-backlog-do-not-implement-now)

---

## 1. Vision & scope

The owner currently uses YouTube Music and hates everything about it except the library. Spotify and Apple Music have better interfaces but lack the obscure material. The goal is:

- **Keep the library depth of YT Music** (obscure, rare, live, cover, video-only uploads)
- **Steal the interface quality of Spotify / Apple Music**
- **Pay nothing, forever.** No subscriptions. No ads. No accounts beyond self-hosted auth.
- **Own the data.** One library on a home server, reachable over Tailscale.

### v1 feature set (implement all of this)

| Feature | Notes |
|---|---|
| Playback core | play/pause, next/prev, seek, queue add/remove/reorder, shuffle (non-repeating), repeat off/all/one, volume |
| Library browse | artists, albums, songs, genres, recently added, recently played, album art |
| Search | fast fuzzy search across artists/albums/songs |
| Playlists | create, rename, delete, add/remove tracks, reorder |
| Likes | star/unstar songs and albums; "Liked" collection |
| System media controls | MediaSession API — lock screen, Control Center, desktop media keys |
| ReplayGain | volume normalization (attenuate-only, see §3.5) |
| Acquisition | paste a YouTube / YT Music URL, or search YT+YTM from the app, and add it to the library |
| Offline downloads | download albums/playlists to the device; play with no network (iPhone + desktop) |
| Responsive UI | mobile layout (tabs) and desktop layout (sidebar), one codebase |
| PWA | manifest, icons, service worker, installable to iPhone home screen |

### Explicit non-goals for v1

See §13. Do not implement them.

---

## 2. Environment — real facts about the machines

Read this before touching anything. These facts were verified. Several of them are **hard policy constraints from the owner's server agent** — violating them will get the work rejected.

### 2.1 Topology

```
fedora-server  (KVM HOST, always-on)          ← PRODUCTION
   tailnet: fedora-server.tailfc5dc.ts.net  /  100.x.x.x
   Docker: INSTALLED
   Role: runs the production stack, holds the real music library
   Deploy root: ~/services/zaymusic/          (the deploying user's home)

devbox  (KVM VM inside fedora-server)         ← DEVELOPMENT
   tailnet: devbox.tailfc5dc.ts.net  /  100.x.x.x
   Hostname: devbox   OS: Fedora Linux 44 (Cloud Edition), SELinux enabled
   Kernel: 7.2.6-200.fc44.x86_64, x86_64, KVM guest
   Resources: 6 cores, 19 GB RAM, 500 GB disk (~485 GB free), btrfs, zstd:1
   sshd: LISTENING on :22 (the host pulls from here)
   Role: development machine. The repository lives here.

iphone (iOS, on tailnet)  100.x.x.x      ← primary client
mypc   (Linux, on tailnet) 100.x.x.x       ← desktop client
```

**The devbox disk is NOT a mount of the host.** It is its own btrfs filesystem. Files written here do not appear on `fedora-server` unless explicitly deployed via the bundle flow in §11.

### 2.2 SSH policy — HARD CONSTRAINT

**There is NO SSH from devbox to `fedora-server`. Do not attempt it. Do not add host keys. Do not ask for host credentials.** The owner's server agent has an explicit standing rule: *no host SSH credentials inside devbox*.

Access runs **only in the safe direction**:

```
   fedora-server  ──ssh──►  devbox        ✅ allowed (host holds ~/.ssh/devbox_ed25519)
   devbox         ──ssh──►  fedora-server ❌ FORBIDDEN, never
```

Consequence: **deployment is pull-based.** devbox prepares a deployable artifact; a manager session on the host pulls it host-side (host-initiated `scp`/`rsync`, no devbox-held host keys) and brings it up with Compose. See §11.

Anything you need on the host must travel **inside the bundle**. There is no remote-exec escape hatch.

### 2.3 Installed tooling

**devbox (this machine):**
| Tool | Status |
|---|---|
| docker + docker-compose | ✅ 29.7.2 |
| podman | ✅ (ignore; use docker) |
| node | ✅ v24.18.0 |
| npm / pnpm / bun | ✅ |
| python3 | ✅ 3.14.7 |
| git | ✅ |
| tailscale | ✅ running |
| sshd | ✅ listening on :22 (for inbound host pulls) |
| ffmpeg | ❌ **not installed** — must live inside the `zayfetch` container |
| yt-dlp | ❌ **not installed** — must live inside the `zayfetch` container |
| caddy / nginx | ❌ not installed — use `tailscale serve` on the host instead |

**fedora-server (host):** Docker is installed (confirmed by the owner). Anything else the stack needs must be satisfied by official container images — do **not** assume host packages exist and do **not** try to install any (we have no shell there).

**Devbox already runs other project containers locally** (e.g. `sbos` on port 8080). Keep doing dev/test in containers here; that is expected and fine.

### 2.4 Host port policy — HARD CONSTRAINT

**Port 8080 on the host is taken by Vaultwarden on all interfaces.**

**Avoid these host ports entirely:** `8080`, `8384`, `22000`, `51515`, `8888`, `3001`.

ZayMusic publishes exactly **one** host port, bound to **loopback only**:

| Service | Host bind | Purpose |
|---|---|---|
| `zaymusic` (SvelteKit) | `127.0.0.1:3847` | the only published port; `tailscale serve` fronts it |
| `navidrome` | *(none)* | internal compose network only |
| `zayfetch` | *(none)* | internal compose network only |

**`APP_PORT=3847`** is the default and is configurable via `.env`. It is deliberately uncommon so it will not collide with anything else on the host. If it ever clashes, change `APP_PORT` — do not go picking ports ad hoc.

Internal container ports (compose network only, never published): `navidrome:4533`, `zayfetch:8080` *(container-internal 8080 is fine — it is not a host port)*, `zaymusic:3000` *(container-internal)*.

> Note: `zayfetch` listens on 8080 *inside* its container. That is unrelated to the host's 8080/Vaultwarden. Only `zaymusic`'s `APP_PORT` is a host port.

### 2.5 Access scope — DECISION

**Tailscale-only.** All clients (iphone, mypc) are already on the tailnet `tailfc5dc.ts.net`. There is no reason to expose ZayMusic to the LAN or the public internet.

- Bind `zaymusic` to `127.0.0.1:${APP_PORT}` on the host.
- `tailscale serve` on `fedora-server` terminates TLS and proxies `https://fedora-server.tailfc5dc.ts.net` → `http://127.0.0.1:3847`.
- **No LAN exposure. No public exposure. No Caddy. No ACME. No custom domain.**

MagicDNS + `tailscale serve` gives a real Tailscale cert, which satisfies the PWA HTTPS requirement with zero DNS/ACME work.

### 2.6 Path layout

**On `fedora-server` (production)** — everything lives under the deploying user's home:

```
~/services/zaymusic/
  docker-compose.yml          # from the bundle
  .env                        # mode 600, real secrets (NEVER committed)
  .env.example                # fake values, from the bundle (safe)
  README-DEPLOY.md            # from the bundle — runbook for the manager session
  apps/web/                   # source, from the bundle (host builds the image)
  services/zayfetch/          # source, from the bundle (host builds the image)
  music/                      # THE music library. persist + BACK UP
  navidrome/                  # Navidrome DB, cache, cover cache. persist + BACK UP
  zayfetch/data/              # job history DB + staging. staging is transient
  app-manifest.json           # which bundle is deployed (written by mkbundle.sh)
```

**On `devbox` (development):**

```
/home/isaiah/Projects/ZayMusic/     # the repository (this project)
  PLAN.md                           # this file
  dev-music/                        # dev music bind. STARTS EMPTY.
                                    # populated ONLY by ZayFetch (yt-dlp).
                                    # There is NO seeding step and there are
                                    # NO bundled/test tracks anywhere. (gitignored)
  dev.env                           # local secrets (gitignored)
  deploy-bundle/                    # OUTPUT of scripts/mkbundle.sh — gitignored
                                    # this is what the host pulls
```

There is **no pre-existing music folder** on the host. `~/services/zaymusic/music/` is the library root and is created on first deploy.

> **There is no such thing as a test track you already own.** The owner has no files to seed. The *only* way audio enters the library is via acquisition (yt-dlp). Do not write a seeding script, do not add fixture audio to the repo, do not gate any milestone on "drop a file in a folder". If you need audio to test with, **go acquire it** — that is also the product's first feature, so testing it is not overhead.

### 2.7 URLs

| Purpose | URL |
|---|---|
| Production app (HTTPS, Tailscale) | `https://fedora-server.tailfc5dc.ts.net` |
| Local dev | `http://localhost:5173` |
| Navidrome — **prod** (compose network only, never published) | `http://navidrome:4533` |
| ZayFetch — **prod** (compose network only, never published) | `http://zayfetch:8080` |
| Navidrome — **dev only** (loopback, via `docker-compose.dev.yml`) | `http://127.0.0.1:4533` |
| ZayFetch — **dev only** (loopback, via `docker-compose.dev.yml`, host port `8081`) | `http://127.0.0.1:8081` |
| Host loopback (only `tailscale serve` uses this) | `http://127.0.0.1:3847` |

---

## 3. Must-know technical constraints

These came out of research. **Violating any of these will produce a broken product.** Read them all before writing the player.

### 3.1 iOS PWA background audio

| Fact | Implication |
|---|---|
| `<audio>` element playback **does** continue in background since iOS 15.4 | Use one plain `<audio>` element for all playback |
| `AudioContext` / `createMediaElementSource` is classified as **"ambient"** and is **killed when the app leaves the foreground** | **NEVER route the playback path through WebAudio.** No EQ, no crossfade, no gain node on the hot path in v1 |
| iOS 26 has a reported bug where a home-screen PWA's `AudioContext` can die permanently until device restart | Extra reason to avoid WebAudio entirely |
| Pause from the lock screen for ~30 seconds → the lock-screen play button becomes dead until the PWA is foregrounded (WebKit, unfixed) | Detect stall and show a "tap to resume" affordance. This is the #1 reason to later wrap in Capacitor |
| Auto-advance at end-of-track can fail in the background (WebKit bug 261858, iOS 16/17) | Preload the next track's `src` so advancing is a single `audio.play()` call |
| MediaSession API works (lock screen, Control Center) | Implement `setActionHandler` for `play`, `pause`, `previoustrack`, `nexttrack`, `seekto`, `seekbackward`, `seekforward`, `stop` |
| iOS blocks autoplay — the **first** play must follow a user gesture | Fine; first play is always a tap |

### 3.2 iOS PWA storage & offline

| Fact | Implication |
|---|---|
| ITP deletes all script-writable storage (IndexedDB, Cache API, localStorage) after **7 days of no interaction** — **but** home-screen PWAs with `display: standalone` or `fullscreen` are **exempt** | Manifest MUST use `display: "standalone"`. Nudge the user to install to the home screen |
| `navigator.storage.persist()` further protects storage and is honored more readily for installed PWAs | Call it early in app boot; surface a one-time dismissible explanation if it is denied |
| Storage quota on iOS 17+ for a home-screen PWA is generous (order of 60% of disk) | Multi-GB offline libraries are viable |
| **OPFS** (Origin Private File System) is supported in Safari / iOS Safari **16.4+** | Use OPFS for audio blobs. Set a minimum supported version of iOS 16.4 |
| OPFS is unavailable in Safari Private Browsing | Feature-detect and show a friendly error |
| Storage pressure can still evict non-persistent data | Show `navigator.storage.estimate()` in a storage meter; let the user purge |

### 3.3 YouTube / yt-dlp audio reality

**The owner does not have (and does not want) YouTube Premium.** Do not build anything that requires it.

| Format ID | Codec | Bitrate | Availability |
|---|---|---|---|
| `251` | Opus | ~130–160 kbps | Free, always. **This is the target format.** |
| `140` | AAC-LC | 128 kbps | Free, always. Fallback. |
| `141` | AAC-LC | 256 kbps | **Premium-gated** and flaky in yt-dlp through 2025–26 (regressions in yt-dlp #13545, #14208, #12891) |
| `774` | Opus | ~256 kbps | **Premium-gated**, same flakiness |

**Format selection:** `-f bestaudio/best`, then keep the native codec. Opus if available, else AAC.

**NEVER use `--audio-format mp3` or any other re-encode.** That is lossy→lossy transcode and degrades quality while inflating size. Only pass `-x` if a container remux is needed; prefer keeping the downloaded stream as-is.

### 3.4 Getting actual songs, not music-video audio

The owner's library includes live sets, covers, remixes, and uploads that exist **only** on plain YouTube (not YouTube Music). So:

- **YouTube and YouTube Music are equal sources.** Query both, merge results.
- **`Artist - Topic` channels are a ranking bonus, never a requirement.** Topic channels are YouTube Music's auto-generated audio-only uploads with static cover art — they are the cleanest source, but most obscure material is not on them.
- **Never hard-filter on title keywords.** A deny-list containing "live", "cover", "remix", "instrumental", "karaoke", "MV" would delete most of this library. These words may only appear as *optional, off-by-default refine chips* that re-sort results. Nothing is ever hidden or rejected automatically.
- The user picks from a candidate list. The app never silently auto-chooses.

Ranking formula is in §9.2.

### 3.5 ReplayGain — the volume constraint

OpenSubsonic exposes a `replayGain` object on every song (`Child`) with: `trackGain`, `albumGain`, `trackPeak`, `albumPeak`, `baseGain`, `fallbackGain` (all in dB). Navidrome populates these from file tags.

**The constraint:** HTML5 `audio.volume` is clamped to `0..1`. You can only **attenuate**, never boost. Boosting requires a WebAudio `GainNode`, which is forbidden on the playback path (§3.1).

**v1 implementation (do this):**
```
gainDb = (mode === 'album' ? albumGain : trackGain) ?? fallbackGain ?? 0
// also fold in baseGain (Ogg Opus output gain) when present
multiplier = 10 ** ((gainDb + (baseGain ?? 0)) / 20)
audio.volume = clamp(volumeSetting * multiplier, 0, 1)   // attenuate-only
```
Tracks with positive gain simply stay at 1.0. This is imperfect but is the only correct option without WebAudio. Modern masters are mostly negative-gain, so it covers the bulk. Expose a settings toggle: `Off | Track | Album`.

ReplayGain tags must be written at **ingest time** (§9.4) so Navidrome can read them.

### 3.6 Navidrome / OpenSubsonic facts

- Navidrome watches the library folder and **auto-imports** new files. ZayFetch just writes files and optionally calls `startScan`.
- OpenSubsonic is a superset of the Subsonic REST API. Navidrome implements both.
- Auth: Subsonic token auth. `token = md5(password + salt)`, send `u`, `t`, `s`, `v`, `c`, `f=json`. See §8.1.
- Playlist mutation (`createPlaylist` / `updatePlaylist` / `deletePlaylist`) is **HTTP POST**, not GET.
- `search3` is the modern search endpoint (ID3-tag-organized). Use it.
- Smart/dynamic playlists exist **server-side** in Navidrome. Do not reimplement them; just surface them in the UI later.
- Last.fm / ListenBrainz scrobbling is a **server-side Navidrome setting**. Turn it on in the Navidrome UI if desired. Do not build client-side scrobbling.

---

## 4. Architecture

```
fedora-server (host, 100.x.x.x) ── PRODUCTION   ~/services/zaymusic/
┌──────────────────────────────────────────────────────────────────┐
│  docker compose                                                  │
│                                                                  │
│  ┌───────────┐   ┌────────────┐   ┌───────────────────────────┐  │
│  │ navidrome │   │  zayfetch  │   │        zaymusic           │  │
│  │ (internal)│   │ (internal) │   │  127.0.0.1:3847  ◄── ONLY │  │
│  │  :4533    │   │   :8080    │   │        published port     │  │
│  │           │   │            │   │                           │  │
│  │ library   │   │ yt-dlp     │   │ SvelteKit (adapter-node)  │  │
│  │ scan      │   │ ytmusicapi │   │  · /rest/*   proxy →      │  │
│  │ OpenSub-  │   │ ffmpeg     │   │      navidrome            │  │
│  │  sonic    │   │ r128gain   │   │  · /api/fetch/*  →        │  │
│  │ API       │   │ musicbrainz│   │      zayfetch             │  │
│  │           │   │  (picard)  │   │  · PWA assets             │  │
│  └─────┬─────┘   └─────┬──────┘   │  · /healthz               │  │
│        │    watches     │ writes   └──────────┬────────────────┘  │
│        │               │                     │                   │
│  ┌─────▼───────────────▼─────────────────────▼────────────────┐  │
│  │   ~/services/zaymusic/music   (bind mount, :z)             │  │
│  └────────────────────────────────────────────────────────────┘  │
│                                                                  │
│  tailscale serve ──► https://fedora-server.tailfc5dc.ts.net      │
│                      (TLS by Tailscale → http://127.0.0.1:3847)  │
└──────────────────────────────────────────────────────────────────┘
            ▲                                ▲
            │        Tailscale only         │
         iphone (PWA)                    mypc (browser)
```

### 4.1 Why this shape

- **Navidrome does the boring hard parts** — library scanning, tag parsing, cover art, transcoding, streaming, playlists, stars, scrobbling, ReplayGain tag reading. Do not reimplement any of it. Write only the client.
- **ZayFetch is separate** because downloads are long-running background jobs (30s–3min each) that need a queue, retries, and status. Do not block a web request on a yt-dlp run.
- **ZayMusic proxies `/rest/*`** so the Navidrome password never reaches the browser and there is no CORS. One origin, one session cookie.
- **`tailscale serve` for TLS** — a real cert with zero ACME/DNS work, and the app is only reachable on the tailnet anyway.
- **Only one published host port, loopback-only.** Navidrome and Zayfetch are unreachable from any network except the compose bridge. This is deliberate.

### 4.2 Dev topology

Same `docker-compose.yml`, **plus** the `docker-compose.dev.yml` override in §4.3. Different `.env`, different (tiny, empty) music bind. Web runs as `pnpm dev` **on the devbox host** for HMR — it is not containerized in dev.

| | devbox | fedora-server |
|---|---|---|
| navidrome + zayfetch | local containers | prod containers |
| zaymusic | `pnpm dev` (:5173) **on the host** | container (host `127.0.0.1:3847`) |
| library bind | `./dev-music/` (**starts empty** — filled only by acquisition) | `~/services/zaymusic/music` |
| navidrome reachable at | `http://127.0.0.1:4533` (published by the dev override) | `http://navidrome:4533` (compose network only) |
| zayfetch reachable at | `http://127.0.0.1:8081` (published by the dev override) | `http://zayfetch:8080` (compose network only) |
| URL | `http://localhost:5173` | `https://fedora-server.tailfc5dc.ts.net` |
| iPhone testing | possible via `tailscale serve` → devbox | **the real one** |
| deploy mechanism | n/a | pull-based bundle, §11 |

### 4.3 Dev-mode networking — REQUIRED

`pnpm dev` runs **on the devbox host**, outside Docker. It cannot resolve compose-network hostnames like `navidrome` or `zayfetch`. Production's `NAVIDROME_URL=http://navidrome:4533` will therefore fail in dev.

**Fix (implement exactly this):**

1. Ship `docker-compose.dev.yml` at the repo root. It is a compose *override*, used only for local dev, and it publishes the two internal services to **loopback** so the host-run SvelteKit can reach them:

```yaml
# docker-compose.dev.yml  — LOCAL DEV ONLY. Never in the deploy bundle.
name: zaymusic
services:
  navidrome:
    ports:
      - "127.0.0.1:4533:4533"     # loopback only
  zayfetch:
    ports:
      - "127.0.0.1:8081:8080"     # loopback only; host 8080 is taken on fedora-server
```

These are **dev-only** and loopback-bound. Production still publishes nothing but `zaymusic`'s `APP_PORT` (§2.5). `mkbundle.sh` must **exclude** `docker-compose.dev.yml` from the bundle (§11.7).

2. `dev.env` uses the **loopback** URLs; `.env` on the host uses the **compose-network** hostnames. This is the whole difference:

| var | `dev.env` (devbox) | `.env` (fedora-server) |
|---|---|---|
| `NAVIDROME_URL` | `http://127.0.0.1:4533` | `http://navidrome:4533` |
| `ZAYFETCH_URL` | `http://127.0.0.1:8081` | `http://zayfetch:8080` |
| `MUSIC_DIR` | `./dev-music` | `./music` |
| `NAVIDROME_DATA_DIR` | `./dev-navidrome` | `./navidrome` |
| `ZAYFETCH_DATA_DIR` | `./dev-zayfetch` | `./zayfetch/data` |
| `APP_PORT` | unused (`pnpm dev` uses 5173) | `3847` |

3. `dev.env.example` (committed, **fake** values) is exactly:

```bash
# dev.env.example — copy to dev.env and run ./scripts/gen-secrets.sh to fill secrets.
# LOCAL DEV ONLY. Never in the deploy bundle.
NAVIDROME_USER=admin
NAVIDROME_PASS=dev-navidrome-pass-change-me
ZAYFETCH_TOKEN=dev-token-change-me

# Dev talks to the dev override on loopback (§4.3). NOT the compose-network names.
NAVIDROME_URL=http://127.0.0.1:4533
ZAYFETCH_URL=http://127.0.0.1:8081

MUSIC_DIR=./dev-music
NAVIDROME_DATA_DIR=./dev-navidrome
ZAYFETCH_DATA_DIR=./dev-zayfetch
APP_PORT=3847
```

4. Bring-up command in dev is always the two-file form:

```bash
docker compose -f docker-compose.yml -f docker-compose.dev.yml --env-file dev.env up -d
```

Do not "fix" this by running SvelteKit inside Docker in dev — you lose HMR. Do not "fix" it by publishing those ports in the base `docker-compose.yml` — that breaks Tailscale-only (§2.5).

---

## 5. Repository layout

```
ZayMusic/                            # /home/isaiah/Projects/ZayMusic
  PLAN.md                            # this file
  README.md                          # short: what it is, how to run dev, how to ship
  NOTES.md                           # open questions / decisions made while implementing
  package.json                       # pnpm workspace root
  pnpm-workspace.yaml
  .gitignore                         # must exclude dev.env, dev-music/, dev-navidrome/,
                                     # dev-zayfetch/, deploy-bundle/, node_modules/, .svelte-kit/
  .env.example                       # every var documented, FAKE values only
  docker-compose.yml                 # THE compose file (prod base; prod via --env-file)
  docker-compose.dev.yml             # DEV-ONLY override: loopback ports for navidrome+zayfetch.
                                     # §4.3. NEVER shipped in the deploy bundle.
  dev.env.example                    # local overrides template (FAKE values). §4.3.

  apps/
    web/                             # SvelteKit PWA
      Dockerfile                     # multi-stage: node:24-slim build → node:24-slim run
      package.json
      svelte.config.js               # @sveltejs/adapter-node
      vite.config.ts
      static/
        manifest.webmanifest
        icons/
      src/
        app.html
        app.d.ts
        lib/
          player/
            engine.ts                # THE core. singleton wrapping ONE <audio>
            queue.ts                 # queue, shuffle, repeat, history
            mediasession.ts          # lock screen / media keys
            replaygain.ts            # gain → audio.volume
          api/
            subsonic.ts              # typed OpenSubsonic client (same-origin /rest)
            fetch-jobs.ts            # ZayFetch client (same-origin /api/fetch)
            input.ts                 # omnibar classifier
            ranking.ts               # candidate scoring client-side mirror
            types.ts                 # Song, Album, Artist, Playlist, Candidate, FetchJob…
          offline/
            opfs.ts                  # audio blob store
            downloads.ts             # download manager + progress
            db.ts                    # Dexie / IndexedDB schema
          stores/
            player.ts
            library.ts
            downloads.ts
            settings.ts
          components/
            Omnibar.svelte
            PlayerBar.svelte
            NowPlaying.svelte
            MiniPlayer.svelte
            TrackList.svelte
            AlbumCard.svelte
            ArtistCard.svelte
            QueueDrawer.svelte
            CandidateList.svelte     # YT/YTM acquisition results
            DownloadButton.svelte
            StorageMeter.svelte
            InstallNudge.svelte
          layouts/
            MobileShell.svelte       # bottom tabs
            DesktopShell.svelte      # sidebar + queue drawer
        routes/
          +layout.svelte
          +layout.ts
          +page.svelte               # Home: recently played / added
          search/+page.svelte        # omnibar results (library + candidates)
          library/
            artists/+page.svelte
            artists/[id]/+page.svelte
            albums/+page.svelte
            albums/[id]/+page.svelte
            songs/+page.svelte
            genres/+page.svelte
            genres/[id]/+page.svelte
          playlists/
            +page.svelte
            [id]/+page.svelte
          liked/+page.svelte
          downloads/+page.svelte     # offline manager + storage meter
          settings/+page.svelte
          api/
            rest/[...path]/+server.ts    # OpenSubsonic proxy
            fetch/[...path]/+server.ts   # ZayFetch proxy
            healthz/+server.ts

  services/
    zayfetch/                        # Python 3.14 service
      Dockerfile                     # python:3.14-slim + ffmpeg + yt-dlp + r128gain
      pyproject.toml                 # PIN yt-dlp version here
      zayfetch/
        __init__.py
        main.py                      # FastAPI: /search /resolve /jobs /healthz
        bootstrap_admin.py           # idempotent Navidrome admin provisioning. §11.3
        ranking.py                   # §9.2 scoring
        downloader.py                # yt-dlp invocation, format selection
        tagger.py                    # metadata write, MusicBrainz repair
        loudness.py                  # r128gain / ReplayGain tags
        mover.py                     # staging → /music, naming
        store.py                     # SQLite job queue
        models.py                    # pydantic schemas
      tests/
        test_ranking.py
        test_mover.py
        test_store.py

    deploy/
      README-DEPLOY.md.tmpl          # THE runbook template. mkbundle.sh copies this to
                                     # deploy-bundle/README-DEPLOY.md. §11.5. This is the
                                     # only authoritative location — do not also put a copy
                                     # under scripts/.

  scripts/
    mkbundle.sh                      # §11.7 — build deploy-bundle/ for the host to pull
    gen-secrets.sh                   # emit a real .env with random secrets. §11.9
    wait-healthy.sh                  # poll https://…/healthz after a deploy. §11.8
    dev-up.sh                        # §4.3 bring-up + pnpm dev
    smoke.sh                         # post-deploy smoke checks (§15)

  deploy-bundle/                     # OUTPUT ONLY. gitignored. §11.4
```

**Language/stack choices (do not deviate without reason):**
- **Web:** SvelteKit 2 + Svelte 5 (runes OK) + TypeScript + Vite. Use `@vite-pwa/sveltekit` or `vite-plugin-pwa` for the service worker. Use `dexie` for IndexedDB. Use plain CSS or Tailwind — pick one and be consistent; Tailwind is fine.
- **Ingest:** Python 3.14 + FastAPI + `yt-dlp` (subprocess is simpler and more debuggable than the library API) + `ytmusicapi` + `ffmpeg` + `r128gain` + `mutagen` + optionally `musicbrainzngs`.
- **DB for jobs:** SQLite via stdlib `sqlite3`. One file. No ORM.
- **No ORM on the web side.** No database on the web side. Everything comes from OpenSubsonic + local IndexedDB.
- **Both images build from `node:24-slim` / `python:3.14-slim` official images only.** No host packages assumed.

---

## 6. Server components

### 6.1 Navidrome

Official image: `deluan/navidrome:latest`. Single user. Nothing is published to the host.

```yaml
# relevant part of docker-compose.yml
services:
  navidrome:
    image: deluan/navidrome:latest
    container_name: zaymusic-navidrome
    restart: unless-stopped
    environment:
      ND_MUSICFOLDER: /music
      ND_DATAFOLDER: /data
      ND_SCANSCHEDULE: 1m
      ND_LOGLEVEL: info
      ND_PORT: 4533
      ND_ENABLETRANSCODINGCONFIG: "true"
      ND_SESSIONTIMEOUT: 30d
    volumes:
      - ${MUSIC_DIR}:/music:z
      - ${NAVIDROME_DATA_DIR}:/data:z
    expose: ["4533"]          # NOT `ports:` — internal only
    networks: [zay]
```

Notes:
- `:z` (SELinux relabel) is **required** on bind mounts. Fedora hosts have SELinux enforcing. Missing this produces baffling permission errors.
- Use `expose:`, never `ports:`, for navidrome and zayfetch.
- **First-run bootstrap problem:** someone must create the Navidrome admin user before `zaymusic` can mint Subsonic tokens. **Do not require a human to click through a UI.** The answer is fixed — implement `services/zayfetch/zayfetch/bootstrap_admin.py`:

  ```
  docker compose run --rm zayfetch python -m zayfetch.bootstrap_admin
  ```

  Contract (acceptance-tested in M0):
  - **Idempotent.** Running it twice is a no-op success. Running it on every deploy is fine.
  - After it exits 0, `NAVIDROME_USER` / `NAVIDROME_PASS` from `.env` authenticate against Navidrome — verify with a Subsonic `ping` using token auth (§8.1) and assert `status == 'ok'`.
  - Implementation sketch: wait for Navidrome's HTTP to answer → if `ping` with the `.env` creds already works, exit 0 → otherwise obtain the first-run admin (Navidrome creates one on an empty DB and reports it in its log / `getUsers`), call `createUser` (and `updateUser` as needed) so the user matches `.env`, then re-verify `ping`.
  - Never print secrets to stdout. Never write a `.env`.

  There is no alternative mechanism. Do not use `ND_ENABLEAUTH=false`, do not tell anyone to click through a UI, do not `docker exec` into Navidrome. This one script, this one command.

### 6.2 ZayFetch (acquisition service)

Python FastAPI service on container-port 8080, **internal only**.

**Container must contain:** `python:3.14-slim` base, `ffmpeg` (apt), `yt-dlp` (pip, **pinned**), `ytmusicapi`, `r128gain`, `mutagen`, `musicbrainzngs`, `fastapi`, `uvicorn`, `httpx`, `rapidfuzz`.

**Responsibilities:**
1. Search YouTube Music + YouTube for a query, return ranked candidates (§9.2).
2. Accept a job to download a specific videoId / URL.
3. Download audio-only, native codec (§3.3).
4. Write clean metadata + embedded cover art.
5. Run ReplayGain analysis and write RG tags.
6. Move the file into `/music` with the canonical path layout (§9.5).
7. POST to Navidrome `startScan` so it appears immediately.
8. Expose job status / retry / cancel.

**Endpoints** — see §8.3.

**Pinning:** pin `yt-dlp==<version>` in `pyproject.toml`. When YouTube breaks it, bump deliberately and test. Do not auto-update inside the container.

### 6.3 ZayMusic (SvelteKit)

`@sveltejs/adapter-node`. Listens on `0.0.0.0:3000` **inside the container**. The compose `ports:` mapping publishes only `127.0.0.1:${APP_PORT}:3000`.

**Server routes:**
- `/rest/[...path]` — proxy to `http://navidrome:4533/rest/...`, injecting Subsonic auth params (§8.1). **Stream** `/rest/stream`, `/rest/download`, `/rest/getCoverArt` through without buffering (§14, item 10).
- `/api/fetch/[...path]` — proxy to `http://zayfetch:8080/...`, adding `X-Zay-Token`.
- `/healthz` — `{ ok, sha, uptime }` where `sha` is read from `/app-manifest.json` (written at bundle build). Used by `wait-healthy.sh` and the smoke script.
- The app has no end-user accounts or sign-in flow. Access is limited at the network layer by the loopback-only port and Tailscale exposure.

---

## 7. Client architecture

### 7.1 Player engine — the critical piece

**Hard rule: exactly one `<audio>` element, never routed through WebAudio.** See §3.1.

```ts
// src/lib/player/engine.ts — shape, not full code
export type RepeatMode = 'off' | 'all' | 'one';

export interface PlayerState {
  track: Song | null;
  playing: boolean;
  position: number;        // seconds
  duration: number;
  volume: number;          // user setting 0..1
  shuffle: boolean;
  repeat: RepeatMode;
  source: 'stream' | 'offline';
}

class PlayerEngine {
  private audio = new Audio();
  // preload the next track so background auto-advance is a single play() call
  private preload = new Audio();

  async load(song: Song, opts?: { seekTo?: number }): Promise<void>;
  play(): Promise<void>;
  pause(): void;
  seek(seconds: number): void;
  setVolume(userVolume: number): void;   // applies ReplayGain, §3.5
  /** Resolve src: OPFS blob URL if downloaded, else /rest/stream?id=… */
  private resolveSrc(song: Song): Promise<string>;
}
```

**Behaviors to implement:**
- `ended` handler advances the queue. Because the next `src` is preloaded into `preload`, advancing swaps `src` and calls `play()` — this is what survives iOS background (§3.1).
- On `error` or a stall while `playing === true`, retry `play()` once after 500 ms, then surface "Tap to resume" (the iOS lock-screen bug, §3.1).
- Persist `{ queue, index, position, shuffle, repeat, volume }` to IndexedDB on every change (debounced 250 ms) so a reload resumes exactly where you were.
- First play must be from a user gesture (iOS autoplay policy). Wire `play()` to a real click/tap in `TrackList` / `PlayerBar`.
- Scrobble via `POST /rest/scrobble?id=…&submission=true` after 50% or 4 minutes, and `submission=false` on start (now-playing). Cheap, and Navidrome forwards to Last.fm if configured.

### 7.2 Queue, shuffle, repeat

`src/lib/player/queue.ts`:
- `queue: Song[]`, `index: number`, `history: Song[]` (for "back")
- **Shuffle must be non-repeating**: build a shuffled *order* (array of indices, Fisher–Yates) and walk it. Reshuffle when exhausted or when the user re-toggles. Do not use `Math.random()` per next-click — that repeats songs.
- Repeat `one`: on `ended`, seek to 0 and play. Repeat `all`: at end of order, rebuild order and continue. Repeat `off`: stop at end.
- Reorder: drag-and-drop on desktop, long-press + drag or explicit "move up/down" on mobile. Keep it simple.

### 7.3 Omnibar — the single search/entry point

**There is exactly one input in the entire app for "find or add music."** No separate "add music" field.

```
┌──────────────────────────────────────────────────────────────┐
│  Search or paste a YouTube link…                         ⌘K  │
└──────────────────────────────────────────────────────────────┘
```

`src/lib/api/input.ts` classifies the input **before** anything else runs:

| Classification | Trigger | UI result |
|---|---|---|
| `video-url` | `youtube.com/watch?v=…`, `youtu.be/…`, `music.youtube.com/watch?v=…`, `music.youtube.com/playlist?list=…` for a single song | One **"Add to library"** card at the top, showing resolved title / channel / duration / badges, with an Add button. Below it, normal library results. |
| `playlist-url` | `youtube.com/playlist?list=…`, `music.youtube.com/playlist?list=…`, `music.youtube.com/browse/…` album | Card expands to a **track list with checkboxes**, then "Add N tracks". |
| `album-url` | `music.youtube.com/browse/` album/EP | Same as `playlist-url`. |
| `library-query` | anything else | Live **library** results (artists / albums / songs) as you type. |

URL detection **always wins** over text search.

**Find-new-music mode:** pressing `Tab` in the omnibar, or clicking a "Find new music" chip in the results, switches the results pane to **YouTube + YT Music candidates** for the current text query, each with an Add button (§9.2, §8.3). `Shift+Tab` returns to library.

Keyboard: `⌘K` / `Ctrl+K` focuses the omnibar from anywhere. `Esc` blurs / clears.

### 7.4 Layouts

**Mobile (< 1024px):** `MobileShell`
- Bottom tab bar: **Home · Search · Library · Playlists · You**
- `MiniPlayer` sits directly above the tab bar (art, title, play/pause, next). Tap → `NowPlaying` full-screen sheet.
- `NowPlaying`: art (large), scrubber, transport, shuffle/repeat, volume, queue button, download button, like button, ReplayGain indicator.

**Desktop (≥ 1024px):** `DesktopShell`
- Left sidebar: Home, Search, Library (Artists / Albums / Songs / Genres), Playlists, Liked, Downloads, Settings.
- Main content column.
- Right `QueueDrawer` (toggleable).
- Persistent `PlayerBar` along the bottom across all routes.

Detect with a CSS media query **and** expose an `isDesktop` store for behavioral differences (keyboard shortcuts, drag-and-drop). Same components, different shells.

**Desktop keyboard shortcuts:** `Space` play/pause, `←/→` seek ±5s, `Shift+←/→` prev/next, `S` shuffle, `R` cycle repeat, `M` mute, `⌘K` omnibar, `/` focus omnibar.

### 7.5 Library browse

Use these OpenSubsonic calls (see §8.1):
- Artists: `getArtists` → `getArtist`
- Albums: `getAlbumList2?type=newest|recent|frequent|random|alphabeticalByName` and `getAlbum`
- Songs: `getAlbum` for album songs, `getSongsByGenre`, `search3` for the long tail
- Genres: `getGenres`
- Recently played: `getAlbumList2?type=recent` plus locally-maintained "played at" in IndexedDB for a proper recents list
- Liked: `getStarred2`
- Cover art: `getCoverArt?id=…&size=300` (and 60 for thumbs). Cache aggressively in the service worker / Cache API.

Render `TrackList` rows: index · art (small) · title · artist · album · duration · like · overflow (add to playlist, download, go to album).

### 7.6 Playlists & likes

- `getPlaylists` / `getPlaylist` / `createPlaylist` (POST) / `updatePlaylist` (POST) / `deletePlaylist` (POST)
- Reordering a playlist = `updatePlaylist` with the full `songIdToAdd` sequence (or remove-all + re-add; check Navidrome's exact semantics and pick the one that works — test it).
- Likes: `star` / `unstar` with `id=` (song) or `albumId=`. Use `getStarred2` for the Liked page. **Do not** mirror into a synthetic "Liked Songs" playlist — `getStarred2` is the source of truth and avoids sync bugs.

---

## 8. Data models & API contracts

### 8.1 Subsonic auth (server-side only)

Every request to Navidrome must carry:

```
u=<username>
s=<random salt, 1..32 chars>
t=<md5(password + salt)>        # hex lowercase
v=1.16.1
c=ZayMusic
f=json
```

The `zaymusic` server route assembles these from `NAVIDROME_USER` / `NAVIDROME_PASS` in env. The browser never sees the password or the token. Use `f=json` everywhere and parse `subsonic-response`. On `status !== 'ok'` throw a typed error with `code`/`message`.

```ts
type SubsonicEnvelope<T> = {
  'subsonic-response': {
    status: 'ok' | 'failed';
    version: string;
    type?: string;          // e.g. "navidrome"
    serverVersion?: string;
    openSubsonic?: boolean;
    error?: { code: number; message: string };
  } & T;
};
```

### 8.2 Core TypeScript types

Define these in `src/lib/api/types.ts`. They map to OpenSubsonic `Child`, `AlbumID3`, `ArtistID3`, `Playlist`, `ReplayGain`.

```ts
export interface ReplayGain {
  trackGain?: number;    // dB
  albumGain?: number;
  trackPeak?: number;    // linear, positive
  albumPeak?: number;
  baseGain?: number;     // e.g. Ogg Opus output gain
  fallbackGain?: number;
}

export interface Song {               // OpenSubsonic Child
  id: string;
  parent?: string;
  isDir: boolean;
  title: string;
  album?: string;
  artist?: string;
  albumId?: string;
  artistId?: string;
  coverArt?: string;
  duration: number;                   // seconds
  track?: number;
  discNumber?: number;
  year?: number;
  genre?: string;
  size?: number;
  suffix?: string;
  contentType?: string;
  bitRate?: number;
  path?: string;
  isVideo?: boolean;
  created?: string;
  starred?: string;
  playCount?: number;
  type?: 'music' | 'podcast' | 'audiobook' | 'video';
  replayGain?: ReplayGain;            // always present per OpenSubsonic
}

export interface Album {
  id: string; name: string; artist?: string; artistId?: string;
  coverArt?: string; songCount: number; duration: number;
  created?: string; year?: number; genre?: string;
}

export interface Artist {
  id: string; name: string; albumCount: number;
  coverArt?: string; artistImageUrl?: string;
}

export interface Playlist {
  id: string; name: string; comment?: string; owner?: string;
  public?: boolean; songCount: number; duration: number;
  created?: string; changed?: string; coverArt?: string;
}
```

ZayFetch types:
```ts
export type Source = 'ytmusic' | 'youtube';

export interface Candidate {
  videoId: string;
  source: Source;
  title: string;
  channel: string;             // "Artist - Topic" or uploader
  durationSec: number;
  thumbnails: { url: string; width: number; height: number }[];
  badges: ('topic' | 'audio-only' | 'likely-mv' | 'has-metadata')[];
  score: number;               // §9.2
  url: string;                 // canonical watch URL
}

export type JobStatus = 'queued' | 'running' | 'done' | 'failed' | 'cancelled';

export interface FetchJob {
  id: string;
  status: JobStatus;
  videoId: string;
  requestedAt: string;
  startedAt?: string;
  finishedAt?: string;
  progress: number;            // 0..1
  stage: 'download' | 'tag' | 'loudness' | 'move' | string;
  error?: string;
  result?: {
    path: string;              // path under /music
    songId?: string;           // Navidrome id once scanned (may be null until scan)
    title: string; artist: string; album: string;
    durationSec: number;
    codec: string;             // 'opus' | 'aac' | …
  };
}
```

### 8.3 ZayFetch HTTP API

All requests carry header `X-Zay-Token: <shared secret>` (from env, also known to `zaymusic`).

```
GET  /healthz                          → { ok: true, yt_dlp: "<version>" }

GET  /search?q=<query>&limit=10        → { candidates: Candidate[] }
   # merges ytmusicapi + yt-dlp ytsearch, ranked per §9.2
   # never filters; only sorts and badges

GET  /resolve?url=<youtube-or-ytm-url> → { kind: 'video'|'playlist'|'album',
                                           candidates: Candidate[],
                                           title?: string }

POST /jobs                             → { job: FetchJob }
   body: { videoId: string, url?: string,
           overrides?: { title?, artist?, album?, track?, year? } }
   # returns immediately with status 'queued'

GET  /jobs?limit=50                    → { jobs: FetchJob[] }   # newest first
GET  /jobs/{id}                        → { job: FetchJob }
POST /jobs/{id}/retry                  → { job: FetchJob }
POST /jobs/{id}/cancel                 → { job: FetchJob }
DELETE /jobs/{id}                      → { ok: true }
```

**`GET /search` must never return an empty result for a query that has hits on YouTube.** If YT Music returns nothing, still return plain YouTube results.

### 8.4 ZayMusic HTTP API (browser-facing)

```
ANY  /rest/*           → proxied to navidrome (service auth injected)
ANY  /api/fetch/*      → proxied to zayfetch (X-Zay-Token)
GET  /healthz          → { ok: true, sha: string, uptimeSec: number }   [public]
```

Everything else is SvelteKit pages.

### 8.5 IndexedDB schema (Dexie)

```ts
// src/lib/offline/db.ts
db.version(1).stores({
  downloads: 'id, songId, state, addedAt',   // id = songId
  playback:  'key',                          // key = 'state', holds PlayerState snapshot
  recents:   'songId, playedAt',
  settings:  'key',
  candidatesCache: 'query, savedAt',
});

interface DownloadRecord {
  id: string;                 // Subsonic song id
  songId: string;
  state: 'queued' | 'downloading' | 'ready' | 'failed';
  progress: number;           // 0..1
  bytes: number;
  opfsPath: string;           // e.g. "audio/<songId>.<ext>"
  addedAt: number;
  error?: string;
  meta: Song;                 // snapshot so the UI works offline
}
```

Audio **bytes** live in OPFS under `audio/<songId>.<ext>`. IndexedDB holds only metadata and state.

---

## 9. Acquisition pipeline (detailed)

### 9.1 Two entry points, one pipeline

| Entry | What happens |
|---|---|
| Omnibar URL paste | `GET /api/fetch/resolve?url=…` → confirm card (or track list) → `POST /api/fetch/jobs` |
| "Find new music" search | `GET /api/fetch/search?q=…` → `CandidateList` → user clicks Add → `POST /api/fetch/jobs` |

The user always sees what will be grabbed (title, channel, duration, badges) and clicks Add. **Never auto-download.** For playlist/album URLs, show a checkbox list first.

### 9.2 Candidate ranking (sort order only — never a filter)

```python
def score(c: Candidate) -> float:
    s = 0.0
    if c.channel.endswith(" - Topic"):        s += 12   # clean audio + static art
    if c.source == "ytmusic":                 s += 6
    s += 5 * fuzzy_ratio(c.title, query)      # 0..1 token sort ratio (rapidfuzz)
    if c.has_embedded_metadata:               s += 3
    if re.search(r"music video|official video|official mv", c.title, re.I):
        s -= 1                                # soft demotion only
    # live / cover / remix / instrumental  →  0  (NEUTRAL — see §3.4)
    if expected_duration and abs(c.durationSec - expected_duration) > 90:
        s -= 4
    return s
```

**Badges shown per row (informational, not gating):**
- `Topic` — channel ends with ` - Topic`
- `Audio-only` — metadata indicates a music/Topic upload with no video track (heuristic: source is `ytmusic` and channel is Topic, or format list has no video stream)
- `Likely MV` — title matches `music video|official video|official mv`
- duration mm:ss · channel name

**Refine chips** (in the `CandidateList` UI) are *client-side re-sorts*, off by default, reversible, and must never remove rows entirely. Suggested chips: `Prefer Topic`, `Hide likely MVs`, `Shortest first`, `Newest first`. If "Hide likely MVs" is on, render those rows **dimmed with a "hidden by filter" affordance and a one-click reveal**, not gone.

### 9.3 Download — format and command

Target: native audio stream, **no re-encode**.

```
yt-dlp \
  --no-playlist \                       # unless the job is a playlist job
  -f "bestaudio/best" \
  --no-progress \
  --newline \
  --embed-metadata \
  --embed-thumbnail \
  --parse-metadata "%(artist)s - %(title)s:%(meta_artist)s - %(meta_title)s" \
  -o "<staging>/%(id)s.%(ext)s" \
  --print-to-file "%(meta_artist)s|%(meta_title)s|%(album)s|%(duration)s|%(ext)s" \
     "<staging>/%(id)s.meta" \
  "https://www.youtube.com/watch?v=<videoId>"
```

Prefer keeping the native container/codec. If a remux is required to embed metadata/thumbnail into a consistent container, remux with `-c copy` (stream copy, no re-encode).

**Do NOT use `--match-filter` as a hard gate.** (§3.4) Any filtering is UI-side only.

**Do NOT use `--cookies` unless absolutely necessary.** Not needed for free-tier audio. If a future feature needs it, store `cookies.txt` at `~/services/zaymusic/zayfetch/data/cookies.txt` on the host and never commit it.

### 9.4 Tagging & loudness

Order matters:

1. **Write tags** with `mutagen` from whatever metadata was resolved (yt-dlp's output, `ytmusicapi` lookup, and only then MusicBrainz as a *repair* — a failed MusicBrainz lookup is not an error). Target tags: `title`, `artist`, `album`, `tracknumber`, `date`, `genre` (if known), `albumartist`.
2. **Embed cover art** (from the thumbnail or from MusicBrainz Cover Art Archive if better art is found).
3. **ReplayGain** with `r128gain` (or `loudgain`) over the final file:
   - Write `REPLAYGAIN_TRACK_GAIN`, `REPLAYGAIN_TRACK_PEAK`, `REPLAYGAIN_ALBUM_GAIN`, `REPLAYGAIN_ALBUM_PEAK` for FLAC/MP3/AAC.
   - For **Opus**, write `R128_TRACK_GAIN` / `R128_ALBUM_GAIN` (and honor `output_gain` as `baseGain`). Navidrome maps these into the OpenSubsonic `replayGain` object. **Verify with one real Opus file that `replayGain` is non-empty in `getSong` before moving on** — if it is empty, the ReplayGain feature is dead and this must be fixed here (M1 acceptance #5).
4. **MusicBrainz repair is optional and last.** Only run if title/artist look wrong (e.g. artist is a channel name like "SomeUser"). Use `musicbrainzngs` with a proper `app`/`version` User-Agent. Never fail a job because MusicBrainz is unreachable.

### 9.5 File naming & move

Canonical path under `/music`:
```
{Artist}/{Album}/{NN} - {Title}.{ext}
```
- Sanitize path components: strip `/ \ : * ? " < > |`, collapse whitespace, trim dots/spaces, cap length at 120 chars per component.
- `{NN}` is zero-padded track number (`01`), or `00` if unknown.
- `{ext}` is the native extension (`opus`, `m4a`, …).
- Unknown artist → `_Unknown Artist`. Unknown album → `_Unknown Album`.
- Write to `/staging/<jobId>/` first, then `os.replace` / `mv` into `/music` **atomically** so Navidrome never scans a partial file.
- After the move, `POST http://navidrome:4533/rest/startScan` (with Subsonic auth) so it appears immediately instead of waiting for the 1-minute schedule.

### 9.6 Job queue

- SQLite table `jobs(id, status, videoId, url, overrides_json, progress, stage, error, requested_at, started_at, finished_at, result_json)` at `/data/jobs.db`.
- A single asyncio worker loop pulls `queued` jobs and runs them **one at a time** (YouTube is rate-limity; parallelism just gets you blocked).
- `running` jobs heartbeat every 5 s. A job with no heartbeat for 60 s is marked `failed` and can be retried.
- yt-dlp progress is parsed from `--newline` output into `progress` (0..1) and `stage: 'download'`, then `tag`, `loudness`, `move`.
- Retries: `POST /jobs/{id}/retry` re-queues a `failed` job. Auto-retry once on transient network errors, then leave it to the user.

---

## 10. Offline subsystem

### 10.1 What "downloaded" means

A track is **ready offline** when its audio bytes are in OPFS and a `DownloadRecord` exists with `state: 'ready'`. Sources are Subsonic IDs so they stay valid.

### 10.2 Downloading to the device

```
UI: DownloadButton (song | album | playlist)
  → for each song: POST /rest/download?id=<songId>   (original file, NOT /stream)
  → stream response body into OPFS at "audio/<songId>.<ext>"
  → record progress in Dexie
  → mark 'ready'
```

Use `/rest/download` (original file) for offline, and `/rest/stream` for online streaming (Navidrome can transcode for cellular — expose a setting `Stream quality: Original | High (320k) | Low (128k)` that maps to `&estimateContentLength` / `&maxBitRate` / `&format`).

`DownloadButton` states: `Not downloaded` · `Downloading 43%` · `Ready offline` · `Failed (retry)`.

### 10.3 Playback resolution order

```ts
async resolveSrc(song: Song): Promise<string> {
  const rec = await db.downloads.get(song.id);
  if (rec?.state === 'ready') {
    const file = await opfs.open(`audio/${song.id}.${rec.ext}`);
    return URL.createObjectURL(await file.getFile());   // blob: URL, works offline
  }
  if (!navigator.onLine) throw new OfflineUnavailable(song);
  return `/rest/stream?id=${song.id}&...`;
}
```

Blob URLs seek fine offline. No service-worker range-request trickery needed.

### 10.4 Storage protection & UX

On first launch:
1. `const granted = await navigator.storage.persist()` — best-effort.
2. If not granted and the user has any downloads, show a one-time dismissible `InstallNudge`: "Add to Home Screen to protect your downloads — iOS deletes web storage after 7 days of inactivity otherwise." (This is true: see §3.2.)
3. Manifest `display: "standalone"` so a home-screen install gets the ITP exemption.

`StorageMeter` on the Downloads page: `navigator.storage.estimate()` → used / quota, plus a per-download list with delete buttons and "Delete all downloads".

### 10.5 Offline UX

- When `!navigator.onLine`, filter browse/search to what is in Dexie/OPFS. Show a subtle "Offline — showing downloads" banner.
- `Downloads` page is browsable fully offline (it reads Dexie).
- If a route can't be satisfied offline, show a friendly empty state with a retry, never a blank screen.

---

## 11. Deployment — pull-based bundle

### 11.1 The model — HARD CONSTRAINT

**devbox never SSHes to the host. Deployment is pull-based.**

```
1. devbox agent: ./scripts/mkbundle.sh
      ──► ~/Projects/ZayMusic/deploy-bundle/     (compose + source + Dockerfiles + .env.example)

2. devbox agent tells the owner:
      "deploy ZayMusic from ~/Projects/ZayMusic/deploy-bundle/"

3. A manager session on fedora-server (host-initiated, host holds the pull key):
      rsync/scp  devbox:~/Projects/ZayMusic/deploy-bundle/  →  ~/services/zaymusic/
      cd ~/services/zaymusic
      # ensure .env exists (mode 600) — see §11.3
      docker compose up -d --build

4. devbox agent: ./scripts/wait-healthy.sh
      polls https://fedora-server.tailfc5dc.ts.net/healthz until sha matches
```

The manager session is **not you**. You cannot run commands on the host. Your deliverable is the bundle plus a clear message to the owner.

**Agent ship procedure:**
```bash
git add -A && git commit -m "M2: player core, queue, library browse"
git push origin main 2>/dev/null || true        # if a remote exists; optional

./scripts/mkbundle.sh                            # produces deploy-bundle/
echo "deploy ZayMusic from ~/Projects/ZayMusic/deploy-bundle/"
# then, after the owner confirms the manager session ran:
./scripts/wait-healthy.sh "$(cat deploy-bundle/app-manifest.json | jq -r .sha)"
./scripts/smoke.sh
```

There is no tag, no git hook, no `post-receive`, no `deploy.sh`. **Do not build any of those.** The bundle + a human message is the entire trigger.

### 11.2 Host deployment handoff — the 5 answers

When the bundle is ready, give these to the owner verbatim (they forward to the host manager session):

| # | Question | Answer |
|---|---|---|
| 1 | **Compose file + image source** | `docker-compose.yml` at the bundle root. **Image source: local build on the host** (`docker compose up -d --build`). Two images, both built from the bundle's own `Dockerfile`s: `zaymusic` (from `apps/web/Dockerfile`, base `node:24-slim`) and `zayfetch` (from `services/zayfetch/Dockerfile`, base `python:3.14-slim` + `ffmpeg`). Third service `navidrome` uses the public image `deluan/navidrome:latest` (no build). **No registry / no GHCR / no image pull credentials needed** beyond Docker Hub for `deluan/navidrome` and the two base images. |
| 2 | **Host port wanted** | Exactly one published port: **`127.0.0.1:3847`** (var `APP_PORT=3847`, configurable in `.env`). Avoids `8080/8384/22000/51515/8888/3001` per policy. Bound to **loopback only** — nothing else on the host or LAN may reach it. `navidrome` and `zayfetch` publish **no** host ports (`expose:` only, compose-internal). |
| 3 | **Env secrets** | Generate with `./scripts/gen-secrets.sh` (ships in the bundle) — it prints a `.env` with real random values. Vars: `NAVIDROME_USER`, `NAVIDROME_PASS`, `ZAYFETCH_TOKEN`, plus `MUSIC_DIR`, `NAVIDROME_DATA_DIR`, `ZAYFETCH_DATA_DIR`, `APP_PORT`. The Navidrome credentials are server-to-server credentials used by the proxy, not app sign-in credentials. Please write to `~/services/zaymusic/.env` with **mode 600**, owner-only. `.env.example` in the bundle has **fake** values only and is safe to commit. **Never commit a real `.env`.** |
| 4 | **Volumes / data that must persist & backup** | **Back up (critical):** `~/services/zaymusic/music/` — the acquired library, irreplaceable. `~/services/zaymusic/navidrome/` — playlists, stars, play counts, users, cover cache. `~/services/zaymusic/.env` — secrets, back up **securely / separately**. **Optional backup:** `~/services/zaymusic/zayfetch/data/jobs.db` — job history (nice "what did I grab" log). **No backup needed:** `~/services/zaymusic/zayfetch/data/staging/` (transient), `~/services/zaymusic/apps/`, `~/services/zaymusic/services/` (re-deployable from the bundle), all container images/volumes. **Please include `music/` and `navidrome/` in nightly backups.** |
| 5 | **Who should reach it** | **Tailscale-only.** No LAN, no public internet. Publish `127.0.0.1:3847` and front it with `tailscale serve` on `fedora-server` so `https://fedora-server.tailfc5dc.ts.net` → `http://127.0.0.1:3847`. All clients (iphone, mypc) are already on `tailfc5dc.ts.net`. |

### 11.3 First-deploy bootstrap (Navidrome admin)

Navidrome needs an admin user before `zaymusic` can mint Subsonic tokens. **Do not require a human to click through a UI.**

The mechanism is fixed — `services/zayfetch/zayfetch/bootstrap_admin.py`, invoked by the manager session as documented in `README-DEPLOY.md` step 5:

```
docker compose run --rm zayfetch python -m zayfetch.bootstrap_admin
```

Full contract and implementation sketch: **§6.1**. Summary: idempotent, no secrets on stdout, exits 0 only after `NAVIDROME_USER`/`NAVIDROME_PASS` from `.env` pass a Subsonic `ping` (§8.1).

There is no alternative. Do not implement an `ND_ENABLEAUTH=false` flip, a UI click-through, or a `docker exec` workaround. Ship this one script.

### 11.4 Bundle contents (`deploy-bundle/`)

Produced by `scripts/mkbundle.sh`. Must be **self-contained** — the host needs nothing except Docker and this directory.

```
deploy-bundle/
  docker-compose.yml          # §11.6
  .env.example                # FAKE values, all vars documented
  README-DEPLOY.md            # §11.5 — the runbook, inlined into the bundle
  app-manifest.json           # { sha, builtAt, slug, zayfetchYtdlpPin, nodeBase, pyBase }
  gen-secrets.sh              # copy of scripts/gen-secrets.sh
  apps/
    web/                      # source + Dockerfile (host builds)
  services/
    zayfetch/                 # source + Dockerfile (host builds)
```

**Must be excluded** from the bundle: `dev.env`, `dev.env.example` (it is dev-only; the bundle ships `.env.example`), `dev-music/`, `dev-navidrome/`, `dev-zayfetch/`, **`docker-compose.dev.yml`** (dev-only override, §4.3), `node_modules/`, `.svelte-kit/`, `build/`, `dist/`, `__pycache__/`, `.pytest_cache/`, `.git/`, `deploy-bundle/` itself, `PLAN.md` (optional — include it if you like, it is harmless and useful documentation), `*.log`, `.env` (real secrets — never).

**Never** put a real `.env` in the bundle. Only `.env.example` with fakes.

### 11.5 `README-DEPLOY.md` — the manager runbook

The template lives at exactly one path: **`services/deploy/README-DEPLOY.md.tmpl`** (see §5). `scripts/mkbundle.sh` copies it verbatim to `deploy-bundle/README-DEPLOY.md` — no substitution, no fallback path, no second copy under `scripts/`. If you need to change the runbook, change that one template.

It must be copy-pasteable and assume the manager is on `fedora-server` with the bundle source at `devbox:~/Projects/ZayMusic/deploy-bundle/`:

```markdown
# ZayMusic — host deploy runbook

Host: fedora-server. Bundle source:  devbox:~/Projects/ZayMusic/deploy-bundle/
Deploy root: ~/services/zaymusic/

## 0. Preconditions
- Docker + docker compose v2 available
- Tailscale running (for tailscale serve)
- Ports 8080/8384/22000/51515/8888/3001 are NOT to be used

## 1. Fetch the bundle (host-initiated pull; no devbox-held host keys)
mkdir -p ~/services/zaymusic
rsync -a --delete \
  devbox:~/Projects/ZayMusic/deploy-bundle/ \
  ~/services/zaymusic/
# (fallback) scp -r devbox:~/Projects/ZayMusic/deploy-bundle/. ~/services/zaymusic/

## 2. Secrets (first deploy only; mode 600)
test -f ~/services/zaymusic/.env || {
  cd ~/services/zaymusic && bash gen-secrets.sh > .env && chmod 600 .env
}
chmod 600 ~/services/zaymusic/.env
# Edit .env if you want to override APP_PORT, NAVIDROME_*, etc.

## 3. Persist dirs
mkdir -p ~/services/zaymusic/{music,navidrome,zayfetch/data/staging}

## 4. Build + start
cd ~/services/zaymusic
docker compose up -d --build

## 5. First deploy only: create the Navidrome admin (idempotent)
docker compose run --rm zayfetch python -m zayfetch.bootstrap_admin

## 6. Expose over Tailscale (loopback only; no LAN/public)
tailscale serve --bg --https=443 http://127.0.0.1:3847
# Verify: curl -fsS https://fedora-server.tailfc5dc.ts.net/healthz

## 7. Backups
Include in nightly backups (critical):
  ~/services/zaymusic/music/
  ~/services/zaymusic/navidrome/
  ~/services/zaymusic/.env        (securely / separately)
Optional:
  ~/services/zaymusic/zayfetch/data/jobs.db
Do NOT back up:
  ~/services/zaymusic/zayfetch/data/staging/
  ~/services/zaymusic/apps/  ~/services/zaymusic/services/   (re-deployable)

## 8. Subsequent deploys
Repeat steps 1, 4. (Skip 2, 3, 5, 6 — all idempotent/skipped.)
Then tell the devbox agent to run ./scripts/wait-healthy.sh && ./scripts/smoke.sh

## Rollback
The previous bundle is not retained automatically. To roll back, ask the devbox
agent to re-run ./scripts/mkbundle.sh against an older git commit and redeploy.
```

### 11.6 `docker-compose.yml` (bundle root)

```yaml
name: zaymusic

services:
  navidrome:
    image: deluan/navidrome:latest
    container_name: zaymusic-navidrome
    restart: unless-stopped
    environment:
      ND_MUSICFOLDER: /music
      ND_DATAFOLDER: /data
      ND_SCANSCHEDULE: 1m
      ND_LOGLEVEL: info
      ND_PORT: 4533
      ND_ENABLETRANSCODINGCONFIG: "true"
      ND_SESSIONTIMEOUT: 30d
    volumes:
      - ${MUSIC_DIR:-./music}:/music:z
      - ${NAVIDROME_DATA_DIR:-./navidrome}:/data:z
    expose: ["4533"]
    networks: [zay]

  zayfetch:
    build:
      context: ./services/zayfetch
    container_name: zaymusic-zayfetch
    restart: unless-stopped
    environment:
      ZAYFETCH_TOKEN: ${ZAYFETCH_TOKEN}
      MUSIC_DIR: /music
      STAGING_DIR: /staging
      DATA_DIR: /data
      NAVIDROME_URL: http://navidrome:4533
      NAVIDROME_USER: ${NAVIDROME_USER}
      NAVIDROME_PASS: ${NAVIDROME_PASS}
    volumes:
      - ${MUSIC_DIR:-./music}:/music:z
      - ${ZAYFETCH_DATA_DIR:-./zayfetch/data}:/data:z
    expose: ["8080"]
    depends_on: [navidrome]
    networks: [zay]

  zaymusic:
    build:
      context: ./apps/web
    container_name: zaymusic-web
    restart: unless-stopped
    environment:
      ZAYFETCH_URL: http://zayfetch:8080
      ZAYFETCH_TOKEN: ${ZAYFETCH_TOKEN}
      NAVIDROME_URL: http://navidrome:4533
      NAVIDROME_USER: ${NAVIDROME_USER}
      NAVIDROME_PASS: ${NAVIDROME_PASS}
      PORT: "3000"
    ports:
      - "127.0.0.1:${APP_PORT:-3847}:3000"   # LOOPBACK ONLY. Tailscale-only exposure.
    depends_on: [navidrome, zayfetch]
    networks: [zay]

networks:
  zay:
```

Notes:
- `zayfetch` uses one bind, `${ZAYFETCH_DATA_DIR}:/data`, and `mover.py` treats **`/data/staging`** as `STAGING_DIR` (a subdirectory of the data bind, created at container start if missing). There is **no second bind for staging** — one mount, one SELinux label, one thing to back up or not. `store.py` puts `jobs.db` at `/data/jobs.db`.
- `ports:` must bind `127.0.0.1`. Do **not** change it to `0.0.0.0` or a bare `${APP_PORT}:3000`. Tailscale-only is a hard requirement (§2.5).
- `:z` on every bind mount (SELinux).
- This is the **prod** file. `docker-compose.dev.yml` (§4.3) is a dev-only override and must not be merged into this file or shipped in the bundle.

### 11.7 `scripts/mkbundle.sh`

```bash
#!/usr/bin/env bash
# Builds ~/Projects/ZayMusic/deploy-bundle/ — the artifact the host pulls.
# Idempotent. Safe to re-run.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$ROOT/deploy-bundle"
STAMP="$(date +%Y.%m.%d-%H%M)"
SHA="$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo "nogit")"

rm -rf "$OUT"
mkdir -p "$OUT"

# source + Dockerfiles (NO dev-only files — see §11.4)
rsync -a \
  --exclude 'node_modules/' --exclude '.svelte-kit/' --exclude 'build/' \
  --exclude 'dist/' --exclude '__pycache__/' --exclude '*.pyc' --exclude '.pytest_cache/' \
  --exclude 'dev-music/' --exclude 'dev-navidrome/' --exclude 'dev-zayfetch/' \
  --exclude 'dev.env' --exclude 'dev.env.example' --exclude '.env' \
  --exclude 'deploy-bundle/' --exclude '.git/' \
  "$ROOT/apps" "$OUT/"
rsync -a \
  --exclude '__pycache__/' --exclude '*.pyc' --exclude '.pytest_cache/' \
  --exclude 'deploy/' \
  "$ROOT/services" "$OUT/"

# prod compose only. NEVER docker-compose.dev.yml (§4.3).
cp "$ROOT/docker-compose.yml" "$OUT/docker-compose.yml"
cp "$ROOT/.env.example" "$OUT/.env.example"
cp "$ROOT/scripts/gen-secrets.sh" "$OUT/gen-secrets.sh"
# the runbook has exactly one source of truth (§11.5)
cp "$ROOT/services/deploy/README-DEPLOY.md.tmpl" "$OUT/README-DEPLOY.md"

# hard guard: the dev override must never ship
rm -f "$OUT/docker-compose.dev.yml" "$OUT/dev.env" "$OUT/dev.env.example"

# record what this bundle is
python3 - <<PY
import json, os, datetime, pathlib
root = pathlib.Path("$OUT")
pin = ""
p = root / "services/zayfetch/pyproject.toml"
if p.exists():
    for line in p.read_text().splitlines():
        if line.strip().startswith("yt-dlp"):
            pin = line.strip()
(root / "app-manifest.json").write_text(json.dumps({
    "sha": "$SHA",
    "slug": "$STAMP",
    "builtAt": datetime.datetime.now(datetime.UTC).isoformat(),
    "zayfetchYtdlpPin": pin,
}, indent=2) + "\n")
PY

# safety: refuse to ship a real .env
if [ -f "$OUT/.env" ]; then
  echo "FATAL: deploy-bundle/.env present — real secrets must never ship" >&2
  rm -f "$OUT/.env"; exit 1
fi

echo "bundle ready: $OUT   (sha=$SHA slug=$STAMP)"
echo "next: tell Isaiah  \"deploy ZayMusic from ~/Projects/ZayMusic/deploy-bundle/\""
```

### 11.8 `scripts/wait-healthy.sh`

Works entirely from devbox (it just curls the Tailscale HTTPS URL). No SSH.

```bash
#!/usr/bin/env bash
# Polls https://fedora-server.tailfc5dc.ts.net/healthz until it reports EXPECT,
# or times out after 5 minutes.
set -euo pipefail
URL="${ZAY_HEALTH_URL:-https://fedora-server.tailfc5dc.ts.net/healthz}"
EXPECT="${1:-}"
[ -n "$EXPECT" ] || { echo "usage: wait-healthy.sh <sha-or-slug>" >&2; exit 2; }
for i in $(seq 1 60); do
  body="$(curl -fsS --max-time 5 "$URL" 2>/dev/null || true)"
  if grep -q "$EXPECT" <<<"$body"; then echo "healthy: $body"; exit 0; fi
  echo "  [$i/60] waiting for '$EXPECT' … last: ${body:-<none>}"
  sleep 5
done
echo "timed out waiting for '$EXPECT' at $URL" >&2
exit 1
```

### 11.9 `scripts/gen-secrets.sh`

```bash
#!/usr/bin/env bash
# Prints a .env to stdout with real random secrets. Redirect to .env, chmod 600.
set -euo pipefail
hex() { openssl rand -hex "${1:-16}"; }
cat <<EOF
# ZayMusic — real secrets. chmod 600. NEVER commit.
NAVIDROME_USER=admin
NAVIDROME_PASS=$(hex 16)
ZAYFETCH_TOKEN=$(hex 32)

# Paths on the host (relative to ~/services/zaymusic/ or absolute)
MUSIC_DIR=./music
NAVIDROME_DATA_DIR=./navidrome
ZAYFETCH_DATA_DIR=./zayfetch/data

# The ONE published host port. Loopback only. Avoid 8080/8384/22000/51515/8888/3001.
APP_PORT=3847
EOF
```

---

## 12. Milestones & acceptance criteria

Work in order. Each milestone is shippable and demoable. **Do not start a milestone before the previous one's acceptance checks pass.**

### M0 — Vertical slice: grab a track and hear it

**The first test is acquisition.** There is no seeded library, no fixture audio, no "drop a file in a folder" step (§2.6). The very first thing that must work is: paste a YouTube URL → ZayFetch pulls it with yt-dlp → Navidrome imports it → you hear it. Everything else is scaffolding around that sentence.

**Build:**
- Infra: `docker-compose.yml`, `docker-compose.dev.yml` (§4.3), both `Dockerfile`s, `.env.example`, `dev.env.example`, `gen-secrets.sh`, `mkbundle.sh`, `services/deploy/README-DEPLOY.md.tmpl`, `wait-healthy.sh`, `smoke.sh`.
- ZayFetch, URL path only: `POST /jobs`, `GET /jobs`, `GET /jobs/{id}`, `GET /resolve?url=…`, `GET /healthz`, plus `bootstrap_admin.py` (§6.1). Full downloader/tagger/loudness/mover (§9.3–§9.5).
- Navidrome up, auto-imports from the (empty) `dev-music/`.
- SvelteKit: `/rest/*` proxy (streaming, §14 item 10), `/api/fetch/*` proxy, `/healthz`.
- **Bare-minimum player:** `PlayerEngine` with `load/play/pause/seek/setVolume` and `resolveSrc` (§7.1, §10.3). No queue, no shuffle, no repeat yet — M2 does those.
- **Omnibar, URL half only:** `input.ts` classifying `video-url` / `playlist-url` / `album-url` vs `library-query` (§7.3), and the `CandidateList` confirm card. Text search / "Find new music" is M1.
- One page: a flat library track list with a play button per row.

**Acceptance:**
1. **devbox, from empty:** `docker compose -f docker-compose.yml -f docker-compose.dev.yml --env-file dev.env up -d` then `docker compose run --rm zayfetch python -m zayfetch.bootstrap_admin` then `pnpm dev`. `dev-music/` is **empty** and there is no seeding step.
2. `./scripts/mkbundle.sh` → `deploy-bundle/` contains `docker-compose.yml`, `.env.example` (fakes only), `README-DEPLOY.md`, both `Dockerfile`s + source, `app-manifest.json`. **Assert the bundle contains NO `.env`, NO `docker-compose.dev.yml`, NO `dev.env.example`, and NO audio files.**
3. Paste a YouTube watch URL into the omnibar at `http://localhost:5173` → confirm card with title/channel/duration → Add → within ~60 s it appears in the library list → **press play and hear it**. This is the milestone. Repeat with a `music.youtube.com` URL and with a single `youtu.be` URL.
4. `python -m zayfetch.bootstrap_admin` exits 0 and is a no-op on the second run. `ping` with `.env` creds returns `ok`.
5. `getSong` on the ingested file returns a **non-empty** `replayGain` (§9.4). If empty, fix `loudness.py` now — the ReplayGain feature is dead otherwise.
6. **fedora-server** (via the owner + manager session, §11.1): bundle deployed to `~/services/zaymusic/`, `docker compose up -d --build` succeeds, `bootstrap_admin` succeeds, `tailscale serve` configured. Then **from the iPhone on cellular**, with `dev-music`-free empty `music/`, paste a YouTube URL in the app → it downloads → **it plays**. `./scripts/wait-healthy.sh <sha>` passes from devbox.

### M1 — Acquisition complete + full omnibar

**Build:** ZayFetch `GET /search` (dual-source, §9.2 ranking), `POST /jobs/{id}/retry|cancel`, `DELETE /jobs/{id}`. Omnibar text mode + "Find new music" (`Tab`) (§7.3). `CandidateList` badges + refine chips (§9.2). Playlist/album URL expansion with checkboxes. MusicBrainz repair polish (§9.4). Job progress UI.

**Acceptance:**
1. Paste a `music.youtube.com/playlist?list=…` URL → checkable track list → add 3 tracks → all three land and play.
2. Search "Find new music" for an obscure track that is **only** on plain YouTube (not YT Music, not a Topic channel) → it appears in the candidates → Add → lands and plays. **No candidate may be silently dropped.**
3. A live recording and a cover are **not** hidden by default; a "Hide likely MVs" chip, when enabled, dims (not removes) rows and can be revealed with one click.
4. `GET /search?q=…` for a query that exists only on YouTube still returns results (the YT-Music-empty case, §8.3).
5. A job that fails (e.g. bad videoId) shows `Failed (retry)` and `retry` actually re-queues it.

### M2 — Player core & browse

**Build:** full `PlayerEngine` (preload-next, stall retry, state persist, scrobble), `queue.ts` (shuffle/repeat/history), library browse pages (§7.5), text search over the library, `MobileShell` + `DesktopShell` (§7.4), `MiniPlayer`, `NowPlaying`, `PlayerBar`, `QueueDrawer`, desktop keyboard shortcuts.

**Acceptance:**
1. Play an album end-to-end. Shuffle 50 tracks and confirm no repeats until the order is exhausted (§7.2).
2. Reload the page mid-track → resumes at the same position with the same queue.
3. Desktop: sidebar layout, keyboard shortcuts work (`Space`, `←/→`, `S`, `R`, `⌘K`).
4. Mobile (<1024px): tab bar + mini player + full-screen now playing.
5. iPhone: lock the screen while playing → lock screen shows art/title and play/pause/next work. (Note: the pause-then-wait-30s resume bug is **known and accepted** — see §3.1. Do not "fix" it with WebAudio.)

### M3 — Collections
**Build:** Playlists CRUD + reorder, likes (`getStarred2`), recents, lightweight tag editor (title/artist/album/track/art) for fixing bad YT metadata.

**Acceptance:**
1. Create a playlist on the phone, add 10 tracks, reorder them, remove one, rename it, delete it.
2. Like 5 songs → they appear under Liked → unlike one → it disappears.
3. Edit a tag on an ugly ingest result → Navidrome reflects it after rescan.

### M4 — System integration & PWA
**Build:** full MediaSession handlers, ReplayGain settings (Off/Track/Album), manifest + icons + service worker, `InstallNudge`, app-shell caching.

**Acceptance:**
1. Lock screen and desktop media keys both drive play/pause/next/prev/seek.
2. ReplayGain `Album` mode is audibly quieter on a loud album than `Off`.
3. Installable from Safari → Add to Home Screen; the app opens with no network after first load.

### M5 — Offline
**Build:** OPFS store, `DownloadButton`, download manager page, `StorageMeter`, offline playback resolution, `navigator.storage.persist()`, offline browse.

**Acceptance:**
1. Download an album on the iPhone. Enable Airplane Mode. Kill and relaunch the PWA. The album plays, seeking works, browse of downloads works.
2. Same on desktop.
3. Storage meter shows a plausible number. Deleting a download frees space.
4. First-launch `persist()` call happens; `InstallNudge` shows once if denied and there are downloads.

### M6 — v1 polish
**Build:** empty/loading/error states everywhere, skeletons, art caching, search-as-you-type debounce + virtualized `TrackList` if >500 rows, `NOTES.md` filled with anything you deferred or decided differently.

**Acceptance:**
1. Every route has a designed empty state and a retry-able error state. No blank screens.
2. Searching a 5k-track library feels instant (measure: type → results < 150 ms from cache).
3. `README.md` explains how to run dev and how to ship (the §11.1 procedure).

**→ v1 shipped.**

---

## 13. Out of scope for v1

Do **not** implement these. They are listed so you do not accidentally build them.

- Gapless playback and crossfade — both need WebAudio on the hot path, which breaks iOS background audio (§3.1). Deferred to a Capacitor-era native audio engine.
- Full ReplayGain including positive boost — same WebAudio reason (§3.5).
- Lyrics (synced or unsynced).
- EQ.
- Scrobbling UI (server-side Navidrome setting handles Last.fm / ListenBrainz).
- Smart/dynamic playlist *editor* (Navidrome has them server-side; surface later).
- CarPlay, widgets, Siri, Apple Watch.
- Multi-user, sharing, public links.
- Any form of payments, accounts, analytics, telemetry.
- Video playback.
- Android (the code is Capacitor-ready so it is nearly free later, but do not test or optimize for it).
- A Capacitor / native shell (the escape hatch is architectural — see §14 — but do not wrap it now).
- **Any git-hook / `post-receive` / tag-based deploy.** That model is forbidden (§11.1). The bundle + a message is the trigger.
- **Any SSH from devbox to `fedora-server`.** Forbidden (§2.2).

---

## 14. Pitfalls — read before writing code

1. **Do not use WebAudio on the playback path.** `createMediaElementSource` moves audio into `AudioContext`, which iOS treats as ambient and kills on background. This single mistake breaks the entire product on iPhone. (§3.1)
2. **Do not `--audio-format mp3`.** Lossy→lossy re-encode. Keep native Opus/AAC. (§3.3)
3. **Do not hard-filter on title keywords.** "live", "cover", "remix", "instrumental", "karaoke" are **neutral**. A deny-list would delete most of this library. Ranking and optional refine chips only. (§3.4)
4. **Do not require `Artist - Topic`.** It is a +12 ranking bonus. Most obscure material is not on Topic channels. (§3.4)
5. **Do not auto-download.** The user always confirms from a candidate list or a resolved URL card.
6. **`audio.volume` cannot boost.** Clamp at 1.0. Do not "fix" it with a GainNode. (§3.5)
7. **SELinux is enforcing on both machines.** Every bind mount in compose needs `:z`. Missing this produces baffling permission errors. (§6.1)
8. **ffmpeg and yt-dlp are not on any host.** They belong inside the `zayfetch` container. Do not apt-install them on devbox or fedora-server.
9. **Playlist mutation in Subsonic is POST**, not GET. GETs are for reads.
10. **Streaming must not be buffered** by the proxy. Pipe `/rest/stream` and `/rest/download` through as a `ReadableStream`. Buffering a 40 MB file will feel broken and can OOM the Node process.
11. **iOS autoplay policy:** the first `play()` must be inside a user-gesture-initiated call stack. Do not try to autoplay on load.
12. **iOS lock-screen pause bug is real and unfixed.** Do not contort the architecture to fix it. Detect, retry once, then show "Tap to resume". Wrap in Capacitor later if it annoys the owner. (§3.1)
13. **Preload the next track** in a second `Audio` element so background auto-advance is a single `play()`. (§3.1)
14. **Pin `yt-dlp`'s version.** YouTube breaks it regularly. Bump deliberately, test, commit.
15. **Write to staging and `os.replace` into `/music`.** Never let Navidrome scan a partially-written file. (§9.5)
16. **Verify ReplayGain actually arrives in the API** as early as M0 (M0 acceptance #5). If Navidrome returns an empty `replayGain` for Opus, fix the tag writing in `loudness.py` before building any client-side normalization.
17. **Persist player state on every change** (debounced). A reload must not lose the queue. The owner will kill the PWA constantly.
18. **Never SSH from devbox to `fedora-server`.** Host policy, hard stop. (§2.2) If you need something on the host, it goes in the bundle.
19. **Never build a git-hook or tag-based deploy.** Forbidden. The deploy trigger is: produce `deploy-bundle/`, tell the owner, wait. (§11.1)
20. **Never publish navidrome or zayfetch host ports.** `expose:` only. Only `zaymusic` gets `ports:`, and only `127.0.0.1:${APP_PORT}:3000`. Tailscale-only is a hard requirement. (§2.5, §11.6)
21. **Never use host port 8080** (Vaultwarden) or `8384 / 22000 / 51515 / 8888 / 3001`. (§2.4)
22. **Never ship a real `.env` in `deploy-bundle/`.** `mkbundle.sh` must fail loudly if one appears. Only `.env.example` with fakes. (§11.4)
23. **Do not install host packages** on fedora-server — you have no shell there and it is not your job. Everything the stack needs must come from official images plus the bundle's own Dockerfiles. (§2.3)
24. **Keep `main` free of deploy coupling.** There is no deploy hook at all. Pushing commits does nothing to production. (§11.1)
25. **Never write a seeding script or ship test audio.** There is no library to seed and no fixtures to commit. Audio enters the world via yt-dlp and nothing else. `dev-music/` and `~/services/zaymusic/music/` both start empty. If a milestone says "drop a file in a folder", that milestone is wrong — go acquire it instead. (§2.6, §12 M0)
26. **Never merge `docker-compose.dev.yml` into `docker-compose.yml`, and never ship it in the bundle.** The dev override publishes navidrome/zayfetch to loopback so host-run `pnpm dev` can reach them (§4.3). In the base file those must stay `expose:`-only or you break Tailscale-only (§2.5). `mkbundle.sh` hard-deletes it from the output.
27. **Do not "fix" the `navidrome` hostname problem by containerizing SvelteKit in dev.** You lose HMR. Use the loopback URLs in `dev.env` and the §4.3 override.

---

## 15. Verification

There is no test framework requirement. Verify like this:

**Automated (cheap, worth it):**
- `services/zayfetch/tests/` — pytest for `ranking.py` (given fixed `Candidate`s, assert order **and that nothing is filtered out** — e.g. a "live" titled result is still returned), `mover.py` path sanitization, job state transitions.
- `apps/web` — vitest for `input.ts` classification (URL vs query vs playlist vs album) and `queue.ts` shuffle non-repetition (run 1000 shuffles of 50 items, assert the first 50 of each order contain no duplicates).
- Typecheck both: `pnpm -w typecheck` and `python -m mypy services/zayfetch` (or just `pytest`).
- A test that `scripts/mkbundle.sh` output: contains `docker-compose.yml`, `.env.example`, `README-DEPLOY.md`, both Dockerfiles, `app-manifest.json`, `gen-secrets.sh`; and contains **no** `.env`, **no** `docker-compose.dev.yml`, **no** `dev.env` / `dev.env.example`, **no** `dev-music/`, and **no** audio files (assert no `*.opus|*.m4a|*.mp3|*.flac|*.wav|*.ogg` anywhere under `deploy-bundle/`).

**Manual (the real gate):**
- Every milestone's acceptance checklist in §12.
- **Always test on the real iPhone on cellular at least once per milestone.** Desktop Chrome passing is not evidence. iOS Safari / home-screen PWA is where the constraints bite.
- For audio behavior (background, lock screen, offline), use a real device, not the simulator.

**Smoke script** (`scripts/smoke.sh`), run from devbox after every deploy. **None of these assume a populated library** — they must pass against an empty install, because that is what a fresh deploy is:
1. `GET /healthz` → 200, `ok: true`, `sha` matches the deployed `app-manifest.json`.
2. `GET /rest/ping` via the proxy (with session) → 200, `status: ok`. This also proves `bootstrap_admin` worked (§6.1).
3. `GET /api/fetch/healthz` → 200 and reports the pinned yt-dlp version.
4. `GET /api/fetch/resolve?url=<a known, stable YouTube watch URL>` → 200 with exactly one candidate, sane title/duration. (Network-dependent; if the network is down, report SKIP rather than FAIL.)
5. `GET /api/fetch/search?q=lorem ipsum zzzqqq` → 200 with `candidates: []` (shape check, not a content check).
6. Assert `https://…` is served with a valid cert (it is Tailscale's) and that `http://fedora-server.tailfc5dc.ts.net:8080`, `:4533`, `:8081` are **not** reachable from the tailnet (nothing else exposed).

---

## 16. Post-v1 backlog (do not implement now)

Recorded so the shape of the architecture stays compatible. Implement only after v1 is done and accepted.

**Steal from other apps:**
- Apple Music: synced lyrics, animated album art on now-playing
- Spotify: crossfade + gapless (needs Capacitor/WebAudio), rotating "daylist" mixes, group session on the tailnet, EQ presets per genre
- YT Music: infinite "Your Mix" radio seeded from the library (Navidrome `getSimilarSongs` / `getSimilarSongs2` already does the hard part)
- Last.fm / ListenBrainz: listening reports, yearly wrapped
- Winamp / foobar2000: 10-band EQ (Capacitor-era), per-album ReplayGain, dense "details" view, custom sort keys

**Original ideas to prototype:**
- **Crate digger** — plays 20 s snippets from albums untouched for 2+ years
- **Listening heatmap** — a GitHub-graph of plays; reveals seasons and holes
- **Desert island generator** — "you get 10 albums forever," scored against actual listening
- **Mood tagging** — freeform user tags driving auto-playlists (backed by Navidrome smart playlists)
- **"Why this song"** — shows the `getSimilarSongs` walk that reached the current track
- **Per-genre crossfade presets** — jazz crossfades differently than breakcore (Capacitor-era)
- **Desperation shuffle** — weights by `1 / plays` so neglected tracks surface

**Deployment refinements (only if the bundle flow gets annoying):**
- Push prebuilt images to **GHCR** so the host only pulls and never builds. Requires a GitHub repo + a read token on the host. The bundle then shrinks to `docker-compose.yml` + `.env.example` + `README-DEPLOY.md` with `image: ghcr.io/…` instead of `build:`.
- Keep two named volumes for `music/` and `navidrome/` instead of bind mounts, if backup tooling prefers volumes.

**Architectural escape hatch:** the client is written to be **Capacitor-ready**. No browser-only APIs on the hot path; platform-detect before using anything exotic. Wrapping later should be `npx cap add ios && npx cap sync`, not a rewrite. When that happens, gapless, crossfade, full ReplayGain, EQ, CarPlay, and widgets all unlock.

---

*End of plan. Go build it.*
