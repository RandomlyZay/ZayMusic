> Native migration scope: the web client is retired at the owner’s request.
> Web-specific implementation references below are historical; the behavior
> reference is `93f43ec:apps/web/...` (private pre-squash archive). Current clients are Linux and iPhone;
> their API gateway is `crates/zay-gateway`. Authentication ownership remains as
> documented here. See `docs/native/DECISIONS.md` for current decisions.

# NOTES

Decisions and deviations made while implementing PLAN.md.

## Decisions

- **yt-dlp pin:** `yt-dlp==2026.8.19` (exists on PyPI). PLAN’s example `2025.12.13` is not published under that exact string.
- **Route paths:** OpenSubsonic proxy lives at `/rest/*` (not `/api/rest/*`); health at `/healthz` (not `/api/healthz`).
- **Web Dockerfile context:** compose `context: .` + `dockerfile: apps/web/Dockerfile` (root workspace layout; context `./apps/web` cannot see `pnpm-workspace.yaml`).
- **`BUILD_SHA`:** required on host deploy (README-DEPLOY step 4 writes it into `.env`); `/healthz` reads env or `/app-manifest.json`.
- **Player SSR:** `Audio` is constructed lazily in `ensureAudio()` — never on the server (Node has no `Audio`).
- **ReplayGain on load:** applied inside `applySrcSync` for every new track; `onEnded` prefers preloaded src swap then `audio.play()`.
- **Playlist reorder:** rewritten in place with `createPlaylist?playlistId=` (Navidrome replaces the songs and keeps the id). Delete+recreate is gone: it broke links and YouTube Music import links.
- **Acquisition jobs:** downloading is initiated from music search; there is no standalone jobs panel in Settings.
- **Track overflow:** add-to-playlist (loads playlists on demand) + Edit tags modal → `updateSong`.
- **r128gain:** installed in zayfetch image; `loudness.py` still falls back to neutral tags if analysis fails so Navidrome `replayGain` is never empty.
- **Virtualized TrackList:** deferred (NOTES below).
- **smoke.sh:** checks the proxied Navidrome and ZayFetch endpoints directly.
- **Navidrome first admin:** `POST /auth/createAdmin` with `{"username","password"}` when DB has zero users (`bootstrap_admin.py`). Fast path still pings with env creds.
- **pnpm deploy:** root `.npmrc` sets `inject-workspace-packages=true` so `pnpm deploy --filter @zaymusic/web --prod` works (portable prod `node_modules`; fixes missing `dexie` in image).
- **Local Tailscale:** `tailscale serve --bg --https=8445 http://127.0.0.1:3847` → `https://devbox.tailfc5dc.ts.net:8445` (needed `sudo` / operator).
- **REST proxy encoding:** never forward client `Accept-Encoding`; force `identity` on the upstream call and recompute `Content-Length` for non-stream JSON GETs. undici auto-gzips + decompresses the body while `Content-Length` stays the compressed size, which truncated JSON to 141 bytes ("Unterminated string in JSON"). Streams still pass through without buffering.
- **Playwright e2e:** `apps/web/e2e/smoke.spec.ts` + `playwright.config.ts`; run with `pnpm test:e2e` (expects stack on `E2E_BASE_URL`, default `http://127.0.0.1:3847`). Chromium installed via `npx playwright install chromium` (system deps skipped on Fedora — binary already present in `~/.cache/ms-playwright`).
- **One omnibar trigger (user-facing):** PLAN lists Search in the desktop nav *and* a top-of-sidebar search chrome. That stacked with a page-level “Open search” and library header “Search” buttons → three+ controls for one input. Desktop keeps only the top sidebar field (⌘K / `/`); nav has no Search item. Mobile keeps the bottom Search tab → `/search` (auto-opens omnibar; re-tapping the tab while already there re-opens it; no second button). Empty-state “Add music” CTAs stay (first-run only).

## Follow-up hardening and verification (2026-09-24)

## Follow-up: Premium-first acquisition (2026-09-27)

- Acquisition is YouTube-Music-Premium-first by product decision: `/jobs` returns 412 until cookies are connected; the worker downloads the requested video directly with those cookies. The 2026-09-25 fallback machinery (probe/swap, alternate-upload ladder, background prewarm, Soulseek rescue) is deleted — Premium plays gated label uploads directly, so there is nothing to rescue.
- Connection lives in the app: Settings → YouTube Music → paste a cookies.txt (extension: “Get cookies.txt LOCALLY” while signed in to music.youtube.com). zayfetch stores it 600 at `$DATA_DIR/cookies/youtube.txt` (`YTDLP_COOKIES_FILE` overrides) via `GET/POST/DELETE /youtube/cookies`.
- Device/OAuth sign-in is not an option: YouTube killed yt-dlp's TV-client OAuth flow (`_perform_login` in `extractor/youtube/_base.py` raises “Login with OAuth is no longer supported”). So the connect flow accepts every format a user can produce with one action: a Netscape cookies.txt (file picker in Settings), a DevTools “Copy as cURL” command, or a raw `Cookie:` header — `downloader.normalize_cookies()` converts them all and rejects blobs without a YouTube sign-in cookie (SID/SSID/HSID/APISID/LOGIN_INFO/__Secure-*SID).
- Verification is two-step, so “invalid cookies” and “free account” are distinguishable: `_cookies_authenticated()` reads YouTube's `"LOGGED_IN":true` page-config flag from `/feed/you`, then a yt-dlp probe against a known Premium-gated track decides the entitlement. Status is cached in `youtube.status.json`; Downloads shows a connect/reconnect banner.
- `yt-dlp` runs with `--js-runtimes node` (nodejs added to the zayfetch image; extraction without a JS runtime is deprecated) plus `--cookies` and `--impersonate chrome`.
- Preview (`/preview/{id}`) attempts the exact video first; on failure without cookies it returns 412 with the connect message instead of silently swapping sources.
- Checks: pytest 144, typecheck 0, vitest 30, e2e 14. Live: job gate 412, fake-cookie probe reports “rejected”, delete returns to unconfigured.

## Follow-up: YouTube-only sources + search page (2026-09-27)

- Soulseek is gone: `soulseek.py`, `/soulseek/search`, the `POST /jobs` soulseek branch, the `soulseek_json` column usage, the slskd compose service/env/volumes, `tagger.scrub_tags`, and the peer-file picker UI are all removed. The app acquires only through the connected YouTube Music account.
- Deezer is out of search and now backs one thing: manual tag repair. `GET /metadata/lookup` returns canonical candidates (title/artist/album/artwork/duration/ISRC) for the Edit-tags dialog's “Look up” button. Search-time catalog verification (`match_all`/`match_all_loose`), Deezer album cards, and the `/album` Deezer branch are deleted; catalog.py keeps only lookup/album parsing plus the upload-title helpers the matcher shared.
- iTunes is gone entirely (`discovery.itunes_lookup`, `CanonicalTrack`, the canonical backstop, `_match_itunes`). `/search` is YTM + YouTube with pure YouTube-signal ranking; the “Top result” card and `playBestMatch` verification logic went with it.
- The search modal is gone. Search is a page: the header field, the mobile tab, and Ctrl+K / `/` all navigate to `/search`, which renders the same omnibar inline. `omnibarOpen` is deleted; `SearchEntry` is a link; the home/library empty-state CTAs link to `/search`.
- Checks: pytest 115, vitest 30, typecheck 0/0, e2e 14. Live: `/healthz` has no soulseek field, `/soulseek/search` 404, `/metadata/lookup?q=J. Cole Change` returns the correct Deezer album, `/search` has no canonical field, and the search page renders with filters All/Songs/Albums/Library.

## Follow-up: gated playback fix — EJS + PO token + web_music (2026-09-27)

- Symptom: with Premium cookies connected, Views / 4 Your Eyez Only / ICEMAN tracks failed to play, and public tracks failed too while signed in. It “never failed before” because the old preview fallback streamed a non-gated re-upload — removing the ladder exposed the real extraction problem.
- Root causes, in order: (1) pip-installed yt-dlp does not bundle the EJS challenge solver, and Debian's Node 20 is below yt-dlp's minimum, so the `n` signature never solved; (2) authenticated sessions get SABR-only formats from the `web`/`web_embedded`/`mweb` clients; (3) a GVS PO Token is required, and yt-dlp reported “PO Token Providers: none”.
- Fix: install Deno (yt-dlp's recommended JS runtime, default-enabled) and `yt-dlp[default]` (bundles yt-dlp-ejs); add a `pot-provider` sidecar (`brainicism/bgutil-ytdlp-pot-provider:2.0.0`, internal) plus the `bgutil-ytdlp-pot-provider` plugin; when cookies are present, `ytdlp_cmd` selects `youtube:player_client=web_music` and points the plugin at the provider. `web_music` + GVS token is the combination that returns real audio (m4a 258k / opus 133k) for gated tracks.
- Resilience: preview and job downloads retry once **without cookies** if the authenticated attempt fails, so a stale/unentitled session can never brick public playback. `_reprobe_youtube_status()` refreshes the cached connection status on startup so an old “could not be verified” self-heals after upgrades.
- Note: yt-dlp has no account-index support; it always authenticates as the browser's default Google account. Multi-account users must export cookies from a session where the Premium account is the default.
- Verified live: connection `premium: true`; gated Views, J. Cole Change, and public previews all 200; an ICEMAN job completed (`Drake - Make Them Cry`, opus). Checks: pytest 115, typecheck 0, e2e 14.

## Follow-up: PWA service worker was never registered (2026-09-27)

- Symptom: “Change j cole” showed nothing in the user's installed app, but a fresh client rendered it. Cause: `registerType: 'autoUpdate'` generated `sw.js`, but nothing registered it (vite-plugin-pwa's automatic injection doesn't survive SvelteKit's SSR shell), so the installed PWA kept running a stale bundle — older bundles defaulted the YouTube source off, and YTM's search API omits Premium-only album tracks, so gated songs never appeared.
- Fix: register `/sw.js` explicitly in the root layout with an hourly `registration.update()`; the generated SW's skipWaiting/clientsClaim make a new deploy activate on the next load. Added an e2e guard (`service worker registers so installed apps receive updates`).
- Note: YTM's `filter="songs"` never returns gated ATV tracks — gated songs only appear through the YouTube provider, so the “Sources” toggle must stay on YouTube (default) for them to be discoverable.

## Follow-up: authoritative uploads rank above re-uploads (2026-09-27)

- Symptom: searching “Change j cole” put a re-upload (“J.Cole Change Official Audio” by an unrelated channel) above the official J. Cole upload. The re-upload collected both the `top_artist` mention bonus (+15, artist named in its title) and the clean-audio bonus (+14) while the official “Change” (channel `J. Cole`) only had +15.
- Fix: the clean-audio bonus now requires an **authoritative uploader** — the artist's own channel/Topic (core name match) or a label/aggregator word (`media|records|official|vevo|…`). A plain re-uploader wearing the artist's name in the title no longer gets it. `_core_channel`/`_authoritative_upload` live in ranking.py. Verified: “Change j cole” now returns the official J. Cole upload first (10.9M views) above the re-upload; “hotline bling drake” and remix queries are unchanged. Checks: pytest 115, e2e 15.

## Follow-up: YTM API is called as the connected account (2026-09-27)

- “Hook into Google's search” — we already do: ytmusicapi IS YouTube Music's Innertube API (same search/suggestions/album/radio endpoints and ordering the official client uses); it was just being called anonymously. `discovery.ytmusic_client()`/`ytmusic_call()` now build browser-auth headers from the stored cookies (`SAPISIDHASH`, `x-goog-authuser: 0`, visitor id) so search, suggestions, albums, and watch-playlist radio run as the connected account, with a transparent anonymous fallback if the cookies are stale.
- Finding worth recording: even authenticated, Google's own YTM search does **not** return the official “Change” (J. Cole) — `filter="songs"` omits it and the unfiltered list only has re-upload *videos*. Deep album cuts are only reachable through the album (4 Your Eyez Only → Change), which the app supports via `/album`. So the app is not a worse search than YTM; for those tracks the album view is the path, and the ranking fix above already puts the official upload first when it appears in the YouTube catalog.
- Checks: pytest 115, e2e 15; live suggestions return Google's (“j cole change”, …), “Change j cole” keeps the official upload first.

## Follow-up: album-aware search (2026-09-25)

## Follow-up: search usability round 3 (2026-09-25)

- PlayerBar/MiniPlayer render above the search overlay (z 1100; menus 1200, dialogs 1300 stay on top), hidden while the Now Playing sheet is open; opening the sheet from the mini player leaves search. Search never auto-closes. Deleted the separate mini-player for good.
- Fast search again: expansion does zero probing (explicit flags + 1h cache kept). Playability resolves at play time (`/preview/{id}?artist=&title=&duration=` falls back on gated IDs) and at download time (worker swaps gated album-override jobs upfront). Album opens in ~4s; first gated play warms the fallback cache, plus background prewarm on album open.
- Fallback hard lessons: gating flaps per request (confirm probes twice, in parallel now); Topic copies gate most — try artist/label uploads first; NEVER cache misses (a cached None bricked tracks until restart).
- `curl_cffi` added (pyproject + Dockerfile) and every yt-dlp call uses `--impersonate chrome` via `downloader.ytdlp_cmd()` — mitigates "Sign in to confirm you're not a bot". NOTE 2026-09-25 ~19:50 UTC: YouTube is hard bot-blocking this egress IP (even impersonated); all streaming 502s, search unaffected. Transient — lifts on its own.
- e2e preview mock updated for query-param preview URLs. Checks: pytest 59, typecheck 0, vitest 25, e2e 8/10 (same 2 pre-existing).

## Follow-up: popularity ranking (2026-09-25)

## Follow-up: catalog authority / Deezer-first search (2026-09-25)

- New `services/zayfetch/zayfetch/catalog.py`: server-side Deezer client (no key; `httpx`, 8s timeouts). `lookup(query)` does 1 call (`/search`, 25 tracks: title/artist/album/duration/`rank`/explicit/ISRC/cover) plus a 2nd (`/album/{id}` full ordered tracklist) only on album intent. TTL cache (300s search / 3600s album) + exponential backoff on 429/5xx/quota-envelope; every function is total (never raises) so a catalog outage can't fail `/search`.
- Verification is exact, never fuzzy: alnum-folded title+artist equality (upload decorations seen through, `Artist - Title` prefixes parsed, ` - Topic`/VEVO/Media suffixes stripped — every artist guess comes from the upload's own text, same rule as the MusicBrainz repair) + duration ±5s when both sides know it. Covers (other artists) and mislabeled re-uploads (wrong duration) never verify.
- Wired into `GET /search` at the top level so music/youtube/all paths rank identically for the frontend merge: catalog runs in parallel with providers; matches boost (`+30` strict) and fill authoritative `album`/`explicit` gaps in place; Deezer verifies an existing YTM pin or supplies its own pinned card (`browseId: "deezer:{id}"`, served by `GET /album` from the Deezer tracklist matched against one bounded ytsearch pool — partial by design, only fires when YTM has no card).
- Three things live data forced (all rank-only, all tested): (1) the official Hotline Bling video is 296s for a 267s recording (verified via full yt-dlp extract — video cuts differ), so own-channel YouTube uploads with exact title+artist verify at a second tier (`+18`, ≤60s, YTM rows excluded since YTM's channel is the artist credit, not the uploader); (2) once the catalog names the recording, unverified same-titled uploads keep popularity at half weight (a 24M-view cover no longer outranks the canonical audio); (3) same-titled covers the catalog *also* lists (e.g. Miro Mas's Hotline Bling) verify at `+10` when their artist disagrees with the query's top artist, and the catalog's top recording supplies `top_artist` when YTM yields none (YTM's artist signal flaps per request).
- Untouched guarantees: sort-only ranking (all `test_rank_returns_everything`-style tests still pass, 100 pytest total), `_fallback_video_id`/`preview` self-heal, no re-encodes, MusicBrainz repair as tag backstop. No model changes (`Candidate.album`/`explicit`, opaque `browseId` already covered it); no frontend changes. Spotify second authority skipped — Deezer alone settles all five acceptance queries.
- Live (dev stack, rebuilt zayfetch): `hotline bling` → Official Audio, official video (2.15B), lyric upload, covers below; `Change j cole` → actual song, no Keyshia/Sam Cooke; `iceman drake`/`Drake Views` → pinned card + real tracks; `Drake Passionfruit` → actual song, karaoke below. `deezer:13082992` opens Views (6/20 tracks resolve from one ytsearch pool — backstop only). Checks: pytest 100, typecheck 0, vitest 25, e2e 8/10 (same 2 pre-existing: smoke.spec.ts:136, :273).
- Ops lesson: `docker compose up -d --build zayfetch` once silently kept the old container (output showed Starting without Recreate); `--force-recreate` after source changes until that's understood.

## Follow-up: One Dance hole — backstop + prefix corroboration (2026-09-26)

- Live trace of "One Dance Drake": YTM songs filter + unfiltered search omit the genuine recording entirely (only a 25K-view 227s mislabel under artist "Drake"); a knockoff album ("One Dance" by "Drakee") pinned via fuzzy artist match. iTunes canonical was exactly right (174s feat. Wizkid & Kyla).
- `GET /search` canonical backstop: when iTunes names a recording nothing verifies against, one bounded plain-YouTube `"{artist} - {title}"` search merges in (rank-only — newcomers must verify on merit to float). Verified live: official Drake Media audio #1 (127.2), Conor Maynard cover out of top 8.
- `_match_itunes` now requires channel corroboration for title-prefix artist claims (label channel / Topic / aggregator words) and reads suffix-form "Title - Artist" official-audio uploads; feat.-guest clauses fold away on both sides. Covers wearing "Artist - " prefixes can no longer steal the canonical boost. `match_one` (Kal contract) untouched.
- `is_album_intent` artist gate tightened from fuzzy WRatio to alnum containment — "Drakee" no longer pins.
- Omnibar Best-match tap mirrors the same rules (prefix/suffix pairs, corroboration, feat-strip, ±8s) and refuses with "No verified recording" instead of playing a lookalike.
- Tests: endpoint tests stub discovery for hermeticity; new backstop + corroboration + intent tests. Checks: pytest 111 (later 112), typecheck 0/0, vitest 25, e2e 8/10 (same 2 pre-existing).

## Follow-up: delete 404 + clean-not-explicit Change (2026-09-27)

- Delete 404 root cause: peer files keep their original tags. The Change FLAC was named `00 - Change.flac` but its stale TRACKNUMBER=93 made Navidrome report `93 - Change.flac`, and the resolver only stripped the number from the disk side. Now strips from both sides (same-dir, same-ambiguity rules). Verified live: the bad Change deleted `{"ok":true}`.
- Ingest hygiene: new `tagger.scrub_tags()` wipes all text metadata from Soulseek files (pictures preserved across FLAC/MP3/MP4/OGG), called before `apply_tags` on the Soulseek branch only. Stale compilation album / track 93 / genre can no longer leak into Navidrome.
- Explicit problem: the corizon file came from a `2010-2019 (4,000+Tracks)` compilation folder — classic clean-edit source. Can't detect clean-vs-explicit from a listing, so selection now prefers the album-folder copy: `/soulseek/search` takes canonical `album`/`artist` (frontend sends `canonical[0]`), `_pick` boosts parsed album+artist matches above compilation rips. Verified live: all top 15 for Change are track 6 from `4 Your Eyez Only`, FLACs first, 10 users — the explicit album cut.
- Frontend: Soulseek search chains after /search (one peer search per query, always hinted); stale peer results cleared per keystroke; unused-notice CSS warning removed.
- Checks: pytest 130, typecheck 0/0, e2e 8/10 (same 2 pre-existing).

- Live diagnosis of the stuck Change job found three backend bugs: (1) `Path(name).name` on Linux keeps `\` so peer basenames never matched — new `basename_of()` splits both separators; (2) `_startup` launched the queue worker twice (flag reset + second call) letting two jobs run concurrently — now a single `ensure_worker()`; (3) the `before` file-set blinded `_locate` to already-present files, so retries/re-adds spun forever — size identity now beats recency, plus a pre-enqueue disk check makes re-adds instant without peer traffic.
- `POST /jobs` soulseek branch dedupes: an active job for the same peer file returns the existing job instead of queueing a twin.
- Proven end-to-end on the real file: corizon's 26MB `J. Cole - Change.flac` → tagged, moved to `J. Cole/4 Your Eyez Only/00 - Change.flac`, scanned, searchable in the library.
- UI fixes for the three complaints: Soulseek status pill pinned at the top of results ("Searching Soulseek…" / "N proper files ↓", tap scrolls to the section) instead of a spinner buried below; Soulseek Add now tracks the job to completion ("Getting… peer downloads take minutes" → "Saved to your library" + library refresh + Play now button) instead of a dead "added to queue"; failures surface the peer error with a try-another-file pointer.
- Checks: pytest 127, typecheck 0/0, vitest 25, e2e 8/10 (same 2 pre-existing).

- Soulseek was wrongly gated behind an opt-in checkbox (same pattern as the YouTube rarities toggle). It isn't a fallback — it's the source for real files. Now every text search fires a Soulseek query automatically; peer results stream into their own section below the instant YTM hits, no action needed. Unconfigured slskd just hides the section.
- Best match card grows a second row when the peer network verifies against the canonical recording (title + artist, FLAC preferred): "Get the proper file · FLAC · 8.1 MB" queues the Soulseek download straight into the library pipeline. Tap-to-play stays instant YTM audio (peer transfers take minutes — they can't preview); ownership comes from Soulseek.
- Honest limit: until a Soulseek username/password is in dev.env, Best match for YTM-missing recordings (Change) can only play duration-verified YT re-uploads. The wiring is done; the creds are the last mile.
- Checks: typecheck 0/0, e2e 8/10 (same 2 pre-existing).

- New `slskd` service (official `slskd/slskd:latest` image, internal-only, no peer port published, no shares). Dev loopback `:5032` for debugging. zayfetch mounts its downloads dir read-only as the pickup dir; slskd state under `${SLSKD_DATA_DIR}` (gitignored both envs).
- New `services/zayfetch/zayfetch/soulseek.py` (all total, never raises): filename search (50-response cap, explicit searches only — never auto-fired), audio-only filtering (FLAC > MP3 > OGG, 300KB–250MB), remote-path parsing (`Artist/Album/NN Title.ext`, `Artist - Title`, year prefixes), per-user caps, enqueue → transfer-poll → settle-check → copy-to-staging (never moves slskd's copy).
- `GET /soulseek/search` (unconfigured → `configured:false`, never 500); `POST /jobs` accepts `{soulseek:{username,filename,size}, overrides}` with `slsk-` token ids; `run_job` branches to the Soulseek download, then the identical tag → loudness → move → scan pipeline. Jobs table gained `soulseek_json` via additive migration (old DBs keep working). `GET /healthz` reports `soulseek` bool.
- Omnibar: "Include Soulseek" opt-in checkbox + "Not here? Try Soulseek" fallback button, results section with user/size/format/bitrate + Add-to-queue. No auto-search, no preview (peer transfers are minutes, not seconds).
- Env: `SLSKD_USER`/`SLSKD_PASS` (register once in SoulseekQt), `SLSKD_API_KEY` (random shared server-side), `SLSKD_DATA_DIR`, `SLSKD_DOWNLOADS_DIR` — in `.env.example`, `dev.env.example`, `gen-secrets.sh` (random key), and dev.env slots. Setup hint shown in-app when unconfigured.
- Setup fixes while wiring: slskd refuses to boot unless its downloads/incomplete dirs exist (pre-created on host; `SLSKD_PICKUP_DIR` points at the `downloads/` subdir), dev slskd dirs needed a `chown` (docker created them root-owned). Probe endpoint is `GET /api/v0/application` (no `/state` — 404 otherwise).
- Checks: pytest 124, typecheck 0/0, vitest 25, e2e 8/10 (same 2 pre-existing). Live dev stack (unconfigured): `/healthz` → `soulseek:false`, `/soulseek/search` → `configured:false`, web 200. Real peer test needs Soulseek creds.

- Live trace of "Change j cole": YTM omits the recording (same omission class as One Dance), but the backstop didn't fire — a *different* song (MIDDLE CHILD) verified against iTunes[1], suppressing it. Gate is now the top canonical hit only (`_match_itunes(cands, canonical[:1])`): a verified wrong song no longer blocks the rescue for the named one. Verified live: real Change uploads (332/333s vs canonical 331s) top the list.
- Best-match tap is now two-tier: (1) channel-corroborated verification; (2) uncorroborated prefix rows only at ±4s duration (leaked-audio quorum like Sam/Otar/Kal at 332s for a 331s song plays; a 180s cover for a 174s song refuses). Never plays a lookalike silently; refuses with a pointer to the list as the last resort.

- New `services/zayfetch/zayfetch/discovery.py` (all total, never raises): iTunes Search canonical lookup (free, no key — title/artist/album/artwork/duration/explicit), Last.fm correction + `track.getSimilar` + `artist.getTopTracks` (optional `LASTFM_API_KEY`, empty = tiers skip), YTM `get_search_suggestions` + `get_watch_playlist(radio=True)` graph. Split "what" (canonical) from "where" (playable uploads).
- `GET /search` now also returns `canonical[]` (iTunes, exact title+artist+duration±8s matched rank-only like Deezer, fills album/explicit gaps), `correction` (Last.fm autocorrect when the query parses as artist+title), `suggestions[]` (YTM). All fetched in parallel with providers — outages change nothing.
- New `GET /suggest?q=` (suggestions + canonical + correction) and `GET /radio?videoId=&artist=&title=` (tier 1 YTM radio → tier 2 Last.fm similar resolved via bounded ytsearch → tier 3 artist top-tracks; `sources[]` says which tiers fired).
- Web `Omnibar.svelte` is one unified search: library (instant, pinned on top) + new-music fire together, no mode toggle. Best-match canonical card, did-you-mean correction button, suggestion chips, YouTube opt-in kept. `QueueDrawer` has an Autoplay toggle (persisted in settings, default on); radio tracks labeled `· Radio`. `engine.ts` prefetches radio at ≤2 remaining and extends the queue at end instead of stopping; `player.ts` wires the radio provider (seed videoId parsed from preview URL).
- Env: optional `LASTFM_API_KEY` in `.env.example` / `dev.env.example` / `gen-secrets.sh` template, passed through compose only to zayfetch.
- Checks: pytest 107, typecheck 0/0, vitest 25, e2e 8/10 (same 2 pre-existing: mobile search-entry position, track-action menu). Live dev stack: `hotline bling` → canonical Drake/Views + YTM suggestions; `/radio` → 8 YTM-radio tracks.

- New-music search defaults to YTM + the Deezer authority (`source=music`); plain-YouTube depth is behind an off-by-default "Include YouTube" checkbox in the results pane (`Omnibar.svelte`), plus a one-click "Not here? Search YouTube too" button in the no-results empty state. Toggling re-fires the debounced search with both origins merged by the existing once-merge. No backend changes (the `source=` param already existed); no e2e changes (the search mock intercepts all sources).
- Tradeoff (accepted): obscure/live/rare uploads that exist only on plain YouTube are hidden until the toggle — that depth is exactly what the toggle is for.
- Checks: typecheck 0, vitest 25, e2e 8/10 (same 2 pre-existing), plus a throwaway Playwright script (since removed) proving only `source=music` fires by default and `music+youtube` fire after checking the box.

- Pure fuzzy matching can't tell the official upload from a same-titled cover, and YTM's songs search itself returns mislabeled re-uploads (a 155s 10K-view "Drake - Hotline Bling" imposter). Candidates now carry `views` (YTM's formatted counts parsed, yt-dlp's `view_count` passthrough).
- `ranking.score` adds `5*log10(views+1)` scaled by title match, gated behind title_match >= 0.6 so popular junk can't hijack album queries; plus +8 when the query's top YTM artist matches (`_find_albums` also returns top artist from unfiltered search; threaded into both search paths).
- YouTube's "Artist - Title" prefixes no longer score as fuzzy cousins (`_title_variants` strips one leading prefix for title match + exact checks).
- Verified live: "hotline bling" → official 2.15B-view upload #1 (was: 84s cover, then imposter); "Change j cole" → all top hits are the actual song. Checks: pytest 64.
- Round 2: whole-word term matching (fuzzy "cole" no longer matches via Keyshia Cole; single letters dropped); whole-artist alnum matching for the official bonus; audio uploads preferred for library intent (+14 audio-pref, -6 MV, rank-only); popularity uncapped so giants separate. Live: audio #1, video #2, imposter #3, covers after; "Change j cole" leads with Official Audio.
- Round 3: split frontend calls ranked inconsistently (top_artist only on music path) — now computed on every path and merged identically; MusicBrainz repair rewritten (old code called nonexistent `search_recording`, silently never ran): strips upload decorations, resolves artist+album strictly (artist must be named in upload, recording must match), wired into run_job before tagging, skipped when overrides are complete. Live: Kal row → J. Cole/Change/4YEO; Drake Media row → Drake/Hotline Bling/Views. Checks: pytest 71.
- Round 4: album intent is deterministic, not fuzzy — query naming album+artist pins that card first (`AlbumResult.pinned`, frontend sorts first) and its tracks get +25 to cluster underneath; YTM songs now carry their album tags (also improves Add overrides + NowPlaying display); >20min "songs" (playlists, rants) demoted rank-only. Live: "iceman drake" → ICEMAN card + real tracks, junk below; "views drake" → Views card + title track + album cuts; hotline order preserved. Checks: pytest 73, e2e 8/10 (same 2 pre-existing).

- Album view has no checkboxes anymore: full tracklist with per-track Add plus one "Add entire album" button. Album adds pass `{title, artist, album}` overrides so files land in the right folder.
- New-music results are one interleaved list (albums scored on the same scale via `ranking.album_score`, `AlbumResult.score`); tapping an album opens its tracklist in order.
- Per-track explicit flags come from YTM's album tracklist (fallback: album flag).
- Views/4YEO tracks now play: OLAK playlist IDs are Premium-gated, so expansion swaps them for playable uploads (ytsearch + duration sanity + extract-and-stream probe, confirmed twice since gating flaps; Topic copies deprioritized; results cached 1h). Browser UA on preview/probe fetches (fixes stray googlevideo 403s).
- Exact multi-word title matches get +12 (Hotline Bling #1); single-word queries unchanged so live/cover variants stay neutral.
- Omnibar mini-player removed — previews play through the main PlayerBar/MiniPlayer/NowPlaying (which already supported `previewUrl`); NowPlaying's Add sends the same album overrides.
- Checks: pytest 56, typecheck 0, vitest 25, live previews 200 for Views + 4YEO tracks, e2e 8/10 (same 2 pre-existing failures: mobile search-entry position, track-action menu).

- YTM `filter="songs"` never returns album tracks, and some albums (Views, 4YEO) are API-restricted (no videoIds). `/search` now also queries YTM albums and expands title+artist-matching albums into playable tracks via the album's audio playlist (yt-dlp resolves it); expansion is gated on the query naming the album title and artist so song queries and knockoff albums don't hijack results.
- Ranking: album-match bonus (+16) plus full-query-term coverage bonus (+10) so exact matches ("J. Cole - Change") outrank same-artist junk instead of being truncated by `limit`.
- `/search` also returns `albums[]`; new `GET /album?browseId=` returns the album with tracks in playlist order. Omnibar New-music pane shows an Albums section; opening one shows the tracklist (checkboxes + Add N) instead of auto-playing. Candidate rows show `artist · album · source`.
- Fixed a merge race in Omnibar's New-music mode (parallel music/youtube fetches clobbered each other — exact matches vanished or sank). Now `Promise.allSettled` + single merge.
- Checks: `python3 -m pytest` (52), `pnpm typecheck` (0), `pnpm test` (25), live `Drake Views` / `J Cole 4 Your Eyez Only` / `Hotline Bling` / `J Cole Change` / `Iceman` (Drake's ICEMAN leads albums, opens to 18 tracks). `pnpm test:e2e` 8/10 — the 2 failures (mobile search-entry positioning, track-action menu) are pre-existing UI issues untouched by this change.

- `yt-dlp --embed-thumbnail` failed every download where `bestaudio/best` selected WebM. Thumbnail postprocessing is removed; the worker reads yt-dlp's full info JSON and keeps the cover beside the album.
- Navidrome does not index WebM audio. WebM Opus/Vorbis is remuxed to `.opus` / `.ogg` with `ffmpeg -c:a copy` before tagging and library scan; no audio re-encode.
- Staging and `/music` are separate bind mounts. Atomic moves now fall back to a temp-file copy on the actual `EXDEV` error rather than relying on `st_dev`.
- Search-result Enter no longer queues a download accidentally; queue feedback stays visible, playlist requests report partial failures, and request races cannot replace newer search results. Mobile search has a visible Done control and doesn't consume Tab navigation.
- UI controls use SVG icons and ASCII copy. Track action menus are portaled to the document body, viewport-positioned, and layered above the player and navigation.
- Current checks: `pnpm typecheck` (0 errors/warnings), `pnpm test` (23), `python3 -m pytest` (31), `pnpm test:e2e` (7, including explicit-add, mobile-search, and top-layer track-menu journeys), `pnpm build`, `docker compose ... build zayfetch`, and dev Compose config validation.
- Live pipeline check: retried a previously failing track; job completed, the `.opus` file was tagged and indexed by Navidrome, and `/rest/stream` returned `206 audio/ogg` for a byte-range request.

## Follow-up: one-way YouTube Music playlist import (2026-09-27)

- New `services/zayfetch/zayfetch/importer.py`: lists the connected account's YT Music library playlists via the existing browser-auth `ytmusicapi` session (`get_library_playlists` / `get_playlist`) and mirrors one into a Navidrome playlist. One-way only — ZayMusic never writes back.
- Endpoints (token-gated): `GET /youtube/library/playlists`, `GET /youtube/library/playlists/{id}`, `POST /youtube/imports`, `GET /youtube/imports`, `POST /youtube/imports/{id}/sync`, `DELETE /youtube/imports/{id}`.
- Import behavior (per owner decision): create the local playlist immediately, link tracks already in the library, and queue Premium-first download jobs for the rest. New SQLite tables `imports` / `import_items` (same jobs.db) track mapping + per-track status; a background reconciler (`ensure_reconciler`, 8s) folds finished downloads back in.
- Matching is normalized title+artist fuzzy (rapidfuzz) with a ±25s duration guard, plus an exact YouTube-video-id match read from the `comment` tag yt-dlp writes. The playlist is rewritten (delete+create) to keep YT Music order; the stored Navidrome playlist id is updated. Re-importing the same YTM playlist reuses its import; Remove deletes only the local playlist (already-queued downloads keep going).
- UX lives inside Settings → YouTube Music (owner decision), `YouTubeImport.svelte`: "Show playlists", per-playlist Import, and imported-playlist progress with Refresh/Remove. Requires a live, verified connection — a stored-but-rejected cookie now returns `412` with a reconnect hint instead of a silent empty list.
- Root cause of the first live attempt showing "No playlists found": `discovery._browser_auth()` never sent an `origin`, and ytmusicapi recomputes `SAPISIDHASH` from `"<sapisid> <origin>"` — with origin absent it hashed `"<sapisid> None"`, so every account-scoped call was served as signed out (library playlists/songs empty; search/radio silently fell back to anonymous). Now sends `origin`/`x-origin: https://music.youtube.com` and a browser `user-agent`; connected-account search/radio benefit too. Also parse YTM's formatted counts (`"1,160"`).
- Checks: pytest 135, typecheck 0/0, vitest 30, e2e 16 (new settings import journey). Live: 25 library playlists listed with correct counts; the 18-track "Drake ICEMAN (Full Album)" imported end-to-end against the real account + Navidrome (matched the already-owned track, queued the other 17, created/deleted the playlist with no orphans) — this caught the real `deletePlaylist` bug (Navidrome wants `id`, not `playlistId`).

## Follow-up: self-sustaining Google auth — managed browser + OAuth (2026-09-27)

- Root cause of "sign-in dies and must be re-pasted": it is not a TTL, it is cookie rotation. yt-dlp's own warning says the browser that owns the session invalidates a pasted snapshot. Fix: give the login exactly one owner — a Chromium on the server — and export its cookies seconds before each download.
- New `services/browser/` (`chromium` + `Xvfb` + `x11vnc` + noVNC + `socat`): one real browser behind noVNC on loopback `:6080`; CDP kept internal. zayfetch talks to it via `BROWSER_CDP_URL` (default `http://browser:9222`); `services/zayfetch/zayfetch/session.py` is the *only* CDP driver.
- Three non-obvious Docker/Chromium gotchas, all fixed in `entrypoint.sh`/`session.py`: (1) Chromium 154 ignores `--remote-debugging-address=0.0.0.0` and only binds loopback, so a `socat` bridge publishes `:9222`; (2) its DevTools HTTP server rejects a non-IP `Host` header with HTTP 500 (and builds the WS URL from it), so `session._resolved_cdp_url()` resolves `browser` → container IP before `connect_over_cdp`; (3) `docker stop`/`start` reuses the writable layer, leaving `/tmp/.X99-lock` and a stale profile `SingletonLock`, so the entrypoint clears both before launching Chromium.
- Cookies: `downloader.load_cookiejar`/`cookiejar_to_netscape` use `yt_dlp.cookies.YoutubeDLCookieJar` so `#HttpOnly_` and `expires=0` session cookies survive; `read_cookies_dict` now strips the `#HttpOnly_` prefix (the stored file can now contain it). `has_login_session` mirrors yt-dlp's `LOGIN_INFO` + `SAPISID` test exactly. `build_cmd(..., with_cookies=True)` refreshes the stored file from the browser immediately before every download (freshness guarantee); `with_cookies=False` and browser-off are untouched.
- Keeper (`ensure_session_keeper`, `YOUTUBE_SESSION_TOUCH_MINUTES` default 30, 0 disables): touch → export → write cookies + `youtube.status.json` `browser`/`lastSyncAt`; backs off to 120 min after consecutive failures, never raises. Called from `_startup` after `_reprobe_youtube_status`. Degradation verified live: browser stopped → `browser: disconnected` and public previews still 206; restart → recovers (`browser: signed_out`, since no Google login was performed here).
- OAuth (ytmusicapi device flow): `POST /youtube/oauth/start` (device code cached 600), `POST /youtube/oauth/poll` (`authorization_pending` → `{connected:false,pending:true}`, success writes `$DATA_DIR/youtube/oauth.json` 600 with `expires_at`), `DELETE /youtube/oauth`. `discovery.ytmusic_client()` now prefers OAuth → cookies → anonymous, and a revoked refresh token falls through instead of raising. Env: `YTM_OAUTH_CLIENT_ID`/`_SECRET` (empty = skip); GCP client type must be "TVs and Limited Input devices".
- Status shape extended (`browser`, `lastSyncAt`, `expiresAt`, `expiresInDays`, `oauth`) and mirrored in `types.ts`. Settings: managed-session row (connected / "Browser session ended" + Open remote browser / paste wording), Google API (OAuth) panel with device-code polling, and the paste form moved under "Advanced — manual cookies" when managed. Downloads banner warns on `signed_out`. Manual paste flow (and its e2e) is unchanged when browser mode reports nothing.
- Live: `healthz` clean; `/youtube/cookies` through the web proxy reports `browser` + `oauth`; noVNC 200 and a CDP screenshot confirms Chromium rendering music.youtube.com; public preview 206, gated preview 502 while signed out (expected). `.env.example`/`dev.env.example`/`gen-secrets.sh` carry the new optional vars; `browser/data/` is gitignored and excluded from the deploy bundle. **A one-time interactive Google sign-in in the remote browser is still required on this host** — the plumbing can't create it.
- The "Open remote browser" link first shipped as `http://<current-host>:6080/vnc.html`, which breaks over Tailscale: noVNC is loopback-only and only the app port (3847/8445) is served, so `devbox.tailfc5dc.ts.net:6080` refuses the connection. Added `PUBLIC_REMOTE_BROWSER_URL` (SvelteKit `$env/dynamic/public`) so the link can point at a served address, plus a `tailscale serve --bg --https=8446 http://127.0.0.1:6080` step in the deploy runbook (and `browser/data` to the backups list). Applied live on devbox: `https://devbox.tailfc5dc.ts.net:8446/vnc.html` renders the Chromium desktop and connects.
- Added Chromium `--test-type` to hide the "unsupported command-line flag: --no-sandbox" infobar over the browser window.
- Checks: pytest 152, typecheck 0/0, vitest 30, e2e 17.

## Deferred / simplifications

- **Virtualized TrackList** for >500 rows: not implemented; simple list is fine for current library sizes (revisit at M6 if needed).
- **Offline bulk download of playlists:** albums/songs only; playlist download not wired (add via songs).
- **Offline offline-banner on every route:** `isOnline` store exists but not painted globally yet.
- **ZayFetch auto-retry:** no automatic retry; job management is not exposed in the Settings page.
- **`/healthz` on zayfetch** does not require `X-Zay-Token`.

## Known product constraints (do not “fix”)

- iOS lock-screen pause >30s → “Tap to resume” (WebKit). Not a bug to solve with WebAudio.
- ReplayGain is attenuate-only; positive gain stays at 1.0.
- `audio.volume` only.

## Verification done at implementation time

- `pnpm typecheck` — clean (tsc + svelte-check, 0 errors)
- `pnpm test` — 23 vitest cases (input classify, queue shuffle/repeat)
- `pnpm test:e2e` — original Playwright smoke cases against `http://127.0.0.1:3847` (home library/JSON, player no stack overflow, settings). Chromium via `npx playwright install chromium`.
- `python3 -m pytest` — 24 cases (ranking, mover, store)
- `pnpm build` — adapter-node build succeeds; `sw.js` + `registerSW.js` present
- `docker compose build zayfetch zaymusic` — both images build
- `docker compose config` (+ dev override) — valid; loopback ports 4533/8081 on dev only
- `./scripts/mkbundle.sh` — bundle asserts: no `.env`, no `docker-compose.dev.yml`, no audio files; yt-dlp pin recorded in `app-manifest.json`
- Live stack smoke: `/healthz`, `/rest/*` JSON (plain + gzip client), `/api/fetch/*`; TLS via `tailscale serve` on 8445

## Follow-up: PLAN-UX Phase 1 (2026-10-08)

- **Launch autoplay:** `restore()` loads paused, but a slow YouTube preview fired `stalled` → `onError()` → `audio.play()`. Error recovery now only retries when `wantsPlayback` (set by `play()`, cleared by `pause()`/system pauses); `stalled` is no longer treated as an error; restored tracks use `preload='none'` so launch doesn't start a yt-dlp extraction.
- **Playlist import is additive:** `_sync_playlist` merges missing YTM songs into the existing playlist in place (`createPlaylist?playlistId=` keeps the id) and never removes anything. Owner decision: a pull blindly re-adds any YTM song missing locally, including songs removed on purpose. `POST /youtube/imports/{id}/pull` re-reads YTM and retries failed items; the reconciler never recreates a playlist deleted from the library (only a pull does). Settings shows "Pull new songs"; there is no Remove — deleting the playlist in the Library also forgets its import.
- **200-song cap:** ytmusicapi `get_playlist` defaults to `limit=100`; now `limit=None` (All Songs: 1158 tracks vs 200).
- **OAuth 400s:** YouTube answers every OAuth-client browse call with HTTP 400, so imports silently ran anonymously (library listing and Liked Music failed). `ytmusic_call` now falls through OAuth → browser cookies → anonymous.

## Follow-up: PLAN-UX Phase 2, new-music latency (2026-10-08)

- `zayfetch/preview.py` owns preview URL extraction. It shares one extraction per video between concurrent callers, caches URLs until their `expire=` minus 10 min (1024 entries, LRU), and runs yt-dlp in-process with a per-thread `YoutubeDL`. That instance is rebuilt when the cookies file's mtime changes and does not write cookies back. Any unexpected in-process error falls back to the CLI. Each extraction logs `extract_ms`.
- `POST /preview/warm {videoIds}` (≤5) extracts in a 2-worker background pool. `PlayerEngine.preloadNext()` calls it for the next two queue items, including right after radio tracks are appended. It no longer buffers preview URLs in the preload element.
- Signed-in extraction uses `player_skip=configs` (~1 s saved). Do not also skip `webpage`: signed-in web_music then returns no audio formats.
- Measured: cold 4.3-5.2 s (was 6.5 s), warmed 0.25-0.34 s, cached 0.1 s.

## Follow-up: PLAN-UX Phase 3, Library/Home (2026-10-08)

- **Saved = starred.** Library Albums is `getStarred2().albums` and Artists is `getStarred2().artists`. Albums use the `SaveButton` (+ / green ✓) and artists use a Follow button. Downloading songs never adds their album to the Library.
- **Album saves.** `zayfetch/library.py` with the `album_saves` table. Matching is by normalized album title, then artist (substring match either way). A title match is accepted without an artist match only when exactly one album has that title, so an ambiguous title is never starred.
- **Play contexts.** `playerActions.playList(songs, i, context?)` records `{type, id, name, coverArt, playedAt}` in the Dexie `contexts` table (db v2). The playlist, album and Liked Songs pages pass a context; one-off lists (search, top songs, recommendations) do not. These records drive the Home shortcut grid, Library "Recents" and the sidebar order. The old per-playlist `getPlaylist` fan-out is gone.
- **`GET /home`** pages `search3` with an empty query (about 1,500 songs in 0.2 s per page) and is cached for 10 min. Rediscover uses a date-seeded shuffle and excludes the top songs.
- **New route `/library/liked`.** The desktop sidebar's "Your collection" links are replaced by a "Your Library" list: Liked Songs, then playlists ordered by recent context.

## Follow-up: PLAN-UX Phase 4, design system (2026-10-08)

- `Icon.svelte` maps the app's icon names to `phosphor-svelte/lib/*` deep imports (not the barrel). Add new names to the map.
- Tokens are in `app.css`: `--surface-1..3`, `--hover`, `--on-accent`, `--scrim`, `--liked-gradient`, `--header-tint`, `--radius-sm|md|lg`, `--fs-*`. No warm/brown leftovers remain. `lib/ui/coverColor.ts` averages cover art (weighted toward saturated pixels, clamped dark enough for white text) into `--header-tint` on album and playlist headers, Now Playing and the mini player. Cross-origin art without CORS falls back to the grey default.
- TrackList:
  - Phone (≤640 px): art, title and artist (green heart/download badges) and a single ⋯. The ⋯ menu and the playlist picker are bottom sheets, through CSS overrides on the same elements (the phone rules must stay last in the style block).
  - Desktop: an album column at ≥1024 px; a liked heart is always visible.
  - The menu gained Like and Add to playlist.
- `Marquee.svelte` scrolls overflowing Now Playing text and respects reduced motion.

## Follow-up: automatic Google re-login (2026-10-11)

- The keeper only ever reported "Browser session ended" and waited for the human. Now, when `GOOGLE_EMAIL` + `GOOGLE_PASSWORD` are set (env), the keeper signs the managed Chromium back in on its own the next tick after a `signed_out` check: `session.attempt_relogin()` opens the Google sign-in over CDP, fills email → password → TOTP (`GOOGLE_TOTP_SECRET` + pyotp), trusts `has_login_session` (not the landed URL) as success, and records `lastReloginAt`/`lastReloginState` in youtube.status.json. Throttled by `YOUTUBE_RELOGIN_MIN_MINUTES` (default 60); a success re-exports cookies and re-probes.
- Safety rails: never runs while an `accounts.google.com` tab is open (bow out with `busy` instead of fighting the human in noVNC); unknown challenge pages or human-only prompts (SMS/phone, captcha) stop with `needs_human` and the UI still points at the remote browser; the driver never raises, never logs typed values.
- Status gains `selfRelogin`, `lastReloginAt`, `lastReloginState`; Settings/Downloads signed-out copy switches to "signing you back in automatically" when `selfRelogin`. Credentials live in dev.env/.env like the other secrets (committed files carry empty placeholders only).
- Checks: pytest 237 (new: full scripted sign-in flow incl. TOTP + chooser + needs_human, throttle, keeper integration), typecheck 0/0, vitest 71. Rebuilt zaymusic+zayfetch images; live `/youtube/cookies` shows the new fields with `selfRelogin: false` (credentials not filled yet — the feature is dormant until GOOGLE_EMAIL/PASSWORD are set).

## Follow-up: automatic re-login — live verification (2026-10-11)

- Real-world flow, caught by probes: Google serves (a) an account chooser for the remembered address → (b) the password page (with a passkey promo carrying its own "Next" button — so submit via Enter, not a text-matched button), (c) often an embedded puzzle CAPTCHA ("Type the text you hear or see"), whose "Try another way" escape leads to (d) the challenge-selection page ("Choose how you want to sign in", no inputs at all) → (e) the Authenticator option → (f) the TOTP field. The driver now handles all of these; a rejected password re-renders silently one cycle later, so the human-marker check runs one iteration after the submit and stops after exactly ONE attempt ("relogin stopped: wrong password").
- compose env mangling: the failing live attempts were not a wrong password — the container received 11 chars of a 12-char value (unquoted value in dev.env, e.g. a `#`/`$`-adjacent char). A Google "Wrong password" error broke the same way. Values must be single-quoted in dev.env/.env. After re-quoting: `automatic Google sign-in succeeded` on the first real attempt, cookies exported, Premium verified.
- `_cookies_authenticated()` (www.youtube.com/feed/you `LOGGED_IN`) flakes with consent variants and even caused a false "YouTube rejected these cookies" while gated extractions kept working. Sign-in evidence now prefers the live managed-browser login (session.browser_logged_in); the page scrape is only the fallback for manual-cookie mode, with 3 attempts.
- Live: `browser: connected · verified: True · premium: True`. Checks: pytest 252, typecheck 0/0, vitest 71.
