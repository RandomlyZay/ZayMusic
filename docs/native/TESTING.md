# Testing native clients

The web client has been retired at the owner’s request. PARITY.md records each
feature's implementation and evidence; this file separates automated fixture and
simulator evidence from checks that need the owner's iPhone or mypc hardware.

## iPhone

The latest verified IPA is **98ab134**, built from this branch's full native batch:
https://devbox.tailfc5dc.ts.net:8445/native-downloads/ZayMusic-98ab134.ipa

Enable Tailscale, download the file, then share/import the `.ipa` into LiveContainer.
Select ZayMusic and launch it inside LiveContainer. This direct attachment preserves
the filename and avoids extracting GitHub's outer artifact ZIP on iPhone. No Apple
developer account, extensions, Live Activities or CarPlay are required.

For development testing, set Settings → Server to
`https://devbox.tailfc5dc.ts.net:8445` and restart ZayMusic inside LiveContainer.

It was verified by macOS run 40991220776 at 98ab134, before the repository's history
was squashed; the identical tree is this repository's initial commit. Earlier runs,
PR #4 and its reviews are kept in the owner's private archive repository.
It built with Xcode 26.6 and the iOS 26.5 SDK. iOS 17 is the minimum supported OS,
separate from SDK linkage; iOS 26/27 uses the system glass tab accessory and controls.
All 18 unit tests and 4 UI tests passed (main screens, collection navigation/search,
management screens, queue sheet). The IPA's CRC, arm64 device executable, background
audio mode, absence of signing and native-design metadata were checked. Its direct
HTTPS download matches SHA-256
`5db619333cd756e03dfa0e788c1fbe80a736bfd5cbf983bfa836fffd6ff64c1a`.

The artifact names are `ZayMusic-unsigned-ipa` and `native-simulator-screenshots`.
All 26 exported captures were reviewed: Home, Search (results, Albums filter), Library,
Liked Songs, album/artist collections and menus, playlists (tracks, picker, actions,
restoration), Queue and queue sheet, Now Playing, Downloads, Settings (YouTube,
rejected credential URL), Jobs, Imports, import details and YouTube Playlists. The
Library tab shows the failure badge from launch. Local files are under
`build/ci-98ab134/`; the IPA is retained in `~/.local/share/zaymusic-builds/` on devbox.
Earlier downloads (e6c510e, 9bf7f84) remain published for comparison.

## Linux

The binary, desktop entry and icon are installed on mypc in the user prefix.
Launch **ZayMusic Native Preview** from the application menu; restart after an update.
The default server is the production Tailscale HTTPS origin; the saved server
preference applies after restart. No updater runs.

Run `./scripts/native-install-linux.sh --mypc` on devbox to build and install on
request. GTK4, libadwaita and GStreamer HTTP/Opus/AAC plugins are required. Catalog
and audio live under `$XDG_DATA_HOME/zaymusic`, normally `~/.local/share/zaymusic`.

For devbox testing, set `ZAY_SERVER=https://devbox.tailfc5dc.ts.net:8445`.
Loopback HTTP is supported for local tests. Native writes use a matching Origin
when present, and the gateway refuses cross-site browser requests. No credentials
belong in native settings or test command lines.

## Prioritized device checklist

1. Under LiveContainer, play, lock the phone, pause for over 30 seconds, then resume
   from the lock screen. Repeat after switching to another app.
2. While locked, let several tracks finish through priority and main queues.
   Confirm auto-advance continues without foregrounding the app.
3. Listen to a continuous album on iPhone and mypc, including real Navidrome Opus/AAC.
   Repeat with shuffle/repeat and priority insertions. Silent WAV tests cannot
   establish whether real audio has a gap.
4. Download tracks, albums and playlists; close/reopen; enable airplane mode;
   browse and play offline. Confirm files survive restart.
5. Reach the server over Tailscale on Wi-Fi and cellular. Interrupt and restore
   connectivity during playback and background downloads.
6. On mypc verify MPRIS metadata, play/pause, next/previous, seek and hardware media
   keys while another application has keyboard focus.
7. Check calls, Bluetooth/headphone removal, ReplayGain, failed imports, retries,
   large/concurrently edited playlists and downloads while suspended.
8. On iOS 27 check the glass tabs, mini-player touch targets, collection sheets and
   Now Playing at the phone's display/text sizes.

## Local validation and macOS budget

**All Linux validation runs on devbox. GitHub Actions is macOS-only and manual.**
A normal branch push or PR update starts no hosted runner. The repository is public,
so standard macOS runners no longer use the private-minutes quota; still batch
Apple changes. No paid plan or spending limit was changed. Before requesting an Apple build, run:

```sh
./scripts/native-local-checks.sh all
```

Scopes `native`, `gateway` and `services` run only the affected checks. Native
covers Rust, Swift/UniFFI, platform checks, silent offline playback/gain, playlist
editing and management/queue UI. Gateway covers mock upstream tests and a
container fixture smoke. Services runs the Python container tests. All fixture
checks use fake credentials. There is no web or Node build.

For live dev-only checks, separately run `./scripts/native-linux-smoke.sh` and
`ZAY_SERVER=https://devbox.tailfc5dc.ts.net:8445 cargo run -p zay-core --example playlist_smoke`.
The latter cleans up only its temporary playlist; neither check logs account IDs.

When an IPA build is requested, dispatch the manual workflow:

```sh
gh workflow run native.yml --ref main
```

No dispatch is performed by the local checks or install scripts. The macOS workflow
keeps Rust/SPM/DerivedData caches, cancels superseded builds, uploads the unsigned
IPA for 14 days and screenshot/test-result artifacts for 7 days. Batching changes
before a requested build avoids spending minutes per push. Once green, download
both artifacts, inspect the IPA and screenshots, and publish the direct attachment.

Historical web-era runs (e6c510e and earlier) also ran web/browser tests; those
suites were retired with the web client.

## Current evidence and known risks

Automated evidence on devbox: **114 Rust core tests**, **15 gateway tests** and
nine gateway fixture checks, the generated Swift/UniFFI bridge, **304 Python
service tests**, Linux release/Clippy, and the real silent-WAV offline handoff
(history and per-track gain), playlist CRUD and management/search/queue/badge
fixture drivers. The live dev-gateway smoke covers playback, navigation and MPRIS.
Logs: `build/attention-*.log`, `build/linux-recovery-*.log`; screenshots under
`build/native-screenshots/`. The current Linux release is installed on mypc with
a matching SHA-256.

Backend tag editing is deployed on devbox only. An empty native tag-edit request
was rejected through the live gateway before file lookup; no real library song was
edited. Test tag edits on a disposable track first.

Simulator evidence is fixture data on a loopback server. It establishes layout,
navigation, control wiring and AVQueuePlayer sequencing, not real network, codec
or background behavior. The device checklist above is still required, highest
priority first: LiveContainer background pause over 30 seconds, locked auto-next,
audible gapless playback, airplane-mode offline, Tailscale on Wi-Fi/cellular, and
mypc hardware MPRIS/media keys. None has been verified on physical hardware.

Other known risks:
- LiveContainer may handle background URLSession completion differently from a
  normally installed app; suspended downloads need a device check.
- Simulator UI runs log a SwiftUI warning about a UIKit toolbar added to a hosting
  view (collection navigation and queue sheet). Tests pass; check the toolbars on
  the phone.
- Faint stale player-title pixels appeared under Xvfb; check the mypc renderer.
- Rows marked "code audit" in PARITY.md are implemented but not exercised by an
  automated test (woven shuffle, radio refill, warming, bulk downloads, tag save,
  library filters/sort). Exercise them during the device pass.
