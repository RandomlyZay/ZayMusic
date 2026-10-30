# Native feature parity

Audit of the native clients against the retired web client (historical source:
`git show 93f43ec:apps/web/...`). The web client was removed at the owner's request.
Every web feature is implemented in both native clients. No feature was declared
impossible. Evidence levels used below:

- **Core test**: a Rust regression in `crates/zay-core/tests` (114 pass).
- **Linux driver**: an Xvfb GTK driver that clicks real controls (`scripts/native-linux-*.sh`),
  or the live dev-gateway smoke.
- **Simulator**: XCTest/XCUITest on the macOS run (18 unit, 4 UI tests) plus reviewed screenshots.
- **Code audit**: wired end to end in source but not exercised by an automated test.
- **Device**: needs the owner's iPhone under LiveContainer or mypc hardware (TESTING.md).

| Web feature | Shared core | Linux | iPhone |
|---|---|---|---|
| Server connection via Tailscale gateway | Implemented; core test | Live dev-gateway smoke | Simulator (fixture loopback); Tailscale on device pending |
| Search library, ranking and paging | Mixed sections, 40-item paging; core test | Linux driver (library + discovery) | Simulator (search results, Albums filter) |
| Omnibar corrections, suggestions, empty-query recents, URL resolution | Classification, correction/suggestion model, recents; core test | Code audit. Like the web client, suggestions appear only when a search returns nothing | Code audit; correction helper unit test. Same suggestion rule as web |
| YouTube/YTM search, previews, explicit acquire, new albums | Shared search/resolve/acquire; core test | Linux driver (new-album acquire) | Simulator (mixed rows); acquire by code audit |
| Home top songs, rediscover, recent songs | Shared sectioned view model; core test | Fixture and dev screenshots reviewed | Simulator screenshots (fixture Home) |
| Recent play contexts, up to eight shortcuts | SQLite recency/dedup/bounds; core test | Code audit (shortcut grid with Play) | Code audit (capped at eight) |
| Saved albums, followed artists, library filters/sort/text | Shared keys, filters, sort; core test | Linux driver (save/follow); filters by code audit | Simulator (Follow/Unfollow); filters by code audit |
| Artists, albums, genres, recent and all songs pages | Shared browse pages; core test | Sidebar pages, smoke screenshots | Library links, code audit |
| Album disc/track order, metadata fallback | Core test | Live album via shared catalog | Shared catalog; simulator album screen |
| Liked Songs, like/unlike song/album/artist | Core test (metadata survives queue/history) | Linux driver (unlike, save, follow) | Simulator (Liked Songs) and current-song like unit test |
| Playlist browse/play/create/rename/delete/add/remove/reorder | Seven core tests; live dev CRUD | Linux driver (full CRUD and reorder) | Simulator (full CRUD, restoration) |
| Play/pause, previous/next, seek | Shared queue; core test | Handoff driver and MPRIS smoke | Simulator unit tests; lock screen on device pending |
| Volume, persisted and combined with ReplayGain | Core test | Handoff driver (MPRIS volume × gain) | Code audit; slider in simulator |
| Queue, history and position persisted, restored paused | Stale-clock and resume core tests | Code audit (restore on launch) | Unit test (saved position resumes same track only) |
| Shuffle off/random; repeat off/all/one | Core test | Linux driver (shuffle) | Unit test (track taps keep shuffle except albums) |
| Priority play next/play last/append | Core test | Linux driver (play next into priority lane) | Unit test (queue edits) |
| Queue jump/remove/reorder including priority; queue drawer | Core test | Linux driver (queue dialog) | Simulator (queue sheet, priority removal) |
| Woven fresh-song shuffle sparse/dense | Weave order, cancellation, fallback; core test | Code audit (Fresh shuffle menu) | Code audit (collection menu) |
| Autoplay/radio fetch, dedup, refill, cancellation | Core test | Code audit (toggle, refill) | Code audit (toggle, refill) |
| YouTube preview playback, warm next URLs | Next-two warming; core test | Code audit | Code audit |
| Gapless native playback | Stable shared lookahead | Handoff driver (three offline items advance); audible gaps need listening | Unit test (AVQueuePlayer prepared items); audible gaps need listening |
| ReplayGain attenuation only | Core test | Handoff driver (per-track gain) | Unit test (per-item gain) |
| MPRIS / desktop media keys | Platform | MPRIS calls verified headless; hardware keys on mypc pending | N/A: iOS uses remote commands (next row) |
| Lock screen, remote commands, interruption, route change | Platform | N/A: MPRIS covers desktop | Play/pause/toggle/stop/next/previous/scrub, artwork; interruption unit test. Track buttons are kept instead of web's ±10 s skip (DECISIONS) |
| Background audio, pause >30 s, locked auto-next | Platform | N/A | Audio background mode in IPA; device pending |
| Offline native files and SQLite catalog | Ticketed atomic commits, migration; core test | Handoff driver (offline playback, server unreachable) | Unit test (offline catalog handoff) |
| Download progress, cancel, retry, delete, storage use | Lifecycle and stale-callback core tests | Code audit; screenshots | Simulator (failed download Retry/Remove); background completion on device pending |
| Album/playlist bulk downloads | Ordered batch; core test | Code audit (collection and shown-list actions) | Code audit (background URLSession) |
| Offline browse and airplane mode | Page caches, outage regression; core test | Code audit (offline banner) | Code audit; airplane mode on device pending |
| ZayFetch jobs/imports, failures, retry, filters, badge | Management commands, retries, attention summary; core test | Linux driver (retry/cancel/dismiss, pull/sync, import); badge in its screenshot | Simulator (Jobs, Imports, YouTube Playlists, filters) |
| Managed Google session, OAuth device flow, manual cookies | Whitelisted status, ephemeral codes, cookie privacy; core test | Linux driver (verify, cookies, OAuth start) | Simulator (settings, OAuth cancel) |
| Settings persistence and full surface | Address validation, volume, autoplay, browser URL; core test | Linux driver | Simulator (rejected credential URL, settings) |
| Go to album/artist; tag edits with metadata lookup | Lookup and `/music/{id}/tags`; core and 304 Python tests | Code audit (tags dialog) | Code audit (tags form); helper unit test |
| Current-song like/unlike/add to playlist, Previous restarts after 3 s | Shared playback policy; core test | Code audit; add to playlist from song menus (web player bar had none) | Unit tests (restart rule, like refresh) |
| Failed item and media-server recovery | Epoch/resume invariants; core test | Code audit. Like web, a failed track pauses with a message instead of skipping | Unit tests (media-server reset, failure copy) |
| Scrobbling and recent song history | Epoch guards, thresholds, 50-song history; core test | Handoff driver (history entries) | Unit test (history after auto-advance) |
| Desktop sidebar / mobile tabs and sheets, art, now playing | Shared artwork cache; core test | Screenshots reviewed | Simulator screenshots (glass tabs, accessory, sheets) |
| Install: Linux desktop entry and icon, no auto-updater | N/A | Installed on mypc; SHA-256 verified | N/A |
| Install: unsigned LiveContainer IPA | N/A | N/A | IPA verified and published (TESTING.md) |
| Web install prompts, service worker, browser storage | Retired with web | Replaced by native executable and SQLite/native files | Replaced by native IPA and SQLite/native files |

Platform N/A entries have their counterpart in the adjacent column and do not waive parity.
"Code audit" rows are implemented; their remaining risk is listed in TESTING.md.
