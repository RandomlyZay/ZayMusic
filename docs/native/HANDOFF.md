# Handoff

## Current state

- Source: /home/isaiah/Projects/ZayMusic (main) and the T3 worktree
  /home/isaiah/.t3/worktrees/ZayMusic/native-migration, both tracking origin/main.
- GitHub `RandomlyZay/ZayMusic` is PUBLIC with squashed history on `main` (owner's
  request). The README states it is personal and unlicensed; issues/wiki/projects off.
  Old history, PR #4, reviews and Actions runs: private `RandomlyZay/ZayMusic-archive-2026-10`
  plus the bundle in ~/.local/share/zaymusic-history/. Mac builds:
  `gh workflow run native.yml --ref main` (manual, batched).
- Web client removed (owner override). Historical spec: 93f43ec:apps/web/... in the private archive repo or history bundle.
- Privacy: never read dev.env/.env, real keys/user IDs, browser storage, real settings
  screens or compiled credentials. Fixture data only. No production/fedora-server changes.

## Verified

- 114 Rust core tests, 15 gateway tests plus nine fixture checks, Swift/UniFFI bridge,
  304 Python tests. Linux Clippy/release, handoff, playlist and management drivers
  pass (build/attention-*.log). Linux release installed on mypc, SHA-256 matched.
- Apple run 40991220776 at 98ab134 (archive repo; same tree as the squashed commit): 18 unit and 4 UI tests passed. All 26 screenshots
  were reviewed (build/ci-98ab134/). IPA verified and published:
  https://devbox.tailfc5dc.ts.net:8445/native-downloads/ZayMusic-98ab134.ipa
- PARITY.md is fully audited: every web feature is implemented on both clients, with
  its evidence level. TESTING.md lists the device checklist and known risks.

## Remaining

Only owner device checks remain (TESTING.md): LiveContainer background pause over
30 s, locked auto-next, audible gapless playback, airplane-mode offline, Tailscale
on Wi-Fi/cellular, and mypc hardware media keys. Fix whatever those checks reveal.
The tag-edit endpoint is deployed on devbox only.
