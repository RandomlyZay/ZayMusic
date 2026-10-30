# Handoff

## Current state

- Checkout: /home/isaiah/.t3/worktrees/ZayMusic/native-migration on devbox, branch
  native-migration, pushed at 61a9aa1. Draft PR #4:
  https://github.com/RandomlyZay/ZayMusic/pull/4. Never push or merge main.
- Repository is PUBLIC (owner's request, after a gitleaks audit of all refs, Actions
  logs and PR text). The README notice ("personal, not for others, no license") exists
  only on this branch; main's README gets it when PR #4 merges. Issues/wiki/projects off.
- Web client removed (owner override). Historical spec: git show 93f43ec:apps/web/...
- Privacy: never read dev.env/.env, real keys/user IDs, browser storage, real settings
  screens or compiled credentials. Fixture data only. No production/fedora-server changes.

## Verified

- 114 Rust core tests, 15 gateway tests plus nine fixture checks, Swift/UniFFI bridge,
  304 Python tests. Linux Clippy/release, handoff, playlist and management drivers
  pass (build/attention-*.log). Linux release installed on mypc, SHA-256 matched.
- Apple run 40991220776 at 98ab134: 18 unit and 4 UI tests passed. All 26 screenshots
  were reviewed (build/ci-98ab134/). IPA verified and published:
  https://devbox.tailfc5dc.ts.net:8445/native-downloads/ZayMusic-98ab134.ipa
- PARITY.md is fully audited: every web feature is implemented on both clients, with
  its evidence level. TESTING.md lists the device checklist and known risks.

## Remaining

Only owner device checks remain (TESTING.md): LiveContainer background pause over
30 s, locked auto-next, audible gapless playback, airplane-mode offline, Tailscale
on Wi-Fi/cellular, and mypc hardware media keys. Fix whatever those checks reveal.
The tag-edit endpoint is deployed on devbox only.
