# Dependency baseline

Project dependencies were checked against the official package registries and upstream releases on 2026-10-29. Direct dependencies and compatible transitive dependencies were refreshed; lockfiles are committed. Production was not changed.

## Native libraries

| Crate | Version |
|---|---|
| serde | 1.0.229 |
| serde_json | 1.0.154 |
| rand | 0.10.3 |
| rusqlite | 0.41.1 |
| reqwest | 0.13.5 |
| uniffi | 0.32.2 |
| thiserror | 2.0.21 |
| url | 2.5.8 |
| tokio | 1.54.1 |
| relm4 | 0.11.0 |
| gstreamer | 0.25.4 |

SQLite retains its checked unsigned-integer conversion through the new `fallible_uint` feature. Rand uses its current `RngExt` trait. Reqwest uses the current `rustls` feature; credentials remain on the gateway. UniFFI bindings are regenerated and exercised from Swift.

## Rust gateway and catalog additions

| Crate | Version |
|---|---|
| axum | 0.8.9 |
| md-5 | 0.11.0 |
| unicode-normalization | 0.1.26 |
| time | 0.3.55 |

The web client, Node/pnpm workspace, TypeScript compilers and web build/test
packages have been removed at the owner's request. Their dependency history is
retained in Git. Python remains for acquisition and managed browser integration;
the native core and credential gateway use Rust.

## Python

| Requirement |
|---|
| fastapi>=0.162.0 |
| uvicorn[standard]>=0.54.0 |
| httpx>=0.28.1 |
| pydantic>=2.14.1 |
| rapidfuzz>=3.14.6 |
| mutagen>=1.48.1 |
| ytmusicapi>=1.12.3 |
| yt-dlp[default]==2026.10.26 |
| curl_cffi>=0.17.0 |
| musicbrainzngs>=0.7.1 |
| bgutil-ytdlp-pot-provider>=2.0.2 |
| playwright>=1.64.1 |
| pyotp>=2.10.0 |
| r128gain>=1.0.7 |
| pytest>=9.2.0 |
| pytest-asyncio>=1.4.1 |
| setuptools>=84.2.1 |

ZayFetch images install from `uv.lock` with UV 0.13.6, including r128gain; silent unlocked pip fallbacks were removed. Tests pass in Python 3.15 as well as the retained Python 3.14-compatible development baseline.

## Runtimes, images and CI

- Gateway image: Rust 1.99.1 build stage and non-root Debian trixie-slim runtime.
- Python 3.15.2 image; Deno 2.9.8 with release SHA-256 verification.
- Browser image uses maintained Debian trixie packages.
- Navidrome 0.64.2 and matching bgutil provider/plugin 2.0.2.
- Swift Linux bridge image 6.4.1.
- macOS 26 runners select the newest stable installed Xcode, require an iOS SDK >=26, and select a modern available simulator. The final app metadata is checked for modern SDK linkage, background audio and unsigned packaging. No beta SDK download is required.
- XcodeGen is refreshed through Homebrew; DerivedData caches are separated by SDK.
- Only the macOS job remains on GitHub, triggered by PR events or manual dispatch. The devbox local-check script runs all Linux validation.

| Action | Upstream version | Pinned commit |
|---|---|---|
| actions/checkout | v7.0.1 | 3d3c42e5aac5ba805825da76410c181273ba90b1 |
| actions/cache | v6.1.0 | 55cc8345863c7cc4c66a329aec7e433d2d1c52a9 |
| actions/upload-artifact | v7.0.2 | cf430e030ddbb5b0abf93d22962f4752f3646cd9 |
| Swatinem/rust-cache | v2.9.3 | d1527c8f67ef9af357290475d4af4b0fab8f12e0 |
| dtolnay/rust-toolchain | master | 683b588c487a69d01f6c6ddf40f036ff682cc2c8 |

## Verification and remaining upstream advisory

- 113 shared Rust tests and 15 gateway tests locally; gateway streaming/range and fixture checks pass. The downloadable e6c510e baseline predates the expanded core.
- Generated Swift/UniFFI queue, HTTP, artwork and offline catalog checks on devbox and the Swift 6.4.1 container.
- Five platform-selection/SDK-artifact checks on Linux, plus actual Rust minimum-version checks for all three Apple targets.
- Python 3.15 container: 252 tests.

The former `deepmerge` build-tool advisory disappeared with removal of the web
workspace. This does not imply a fresh security audit of unrelated upstream code.
