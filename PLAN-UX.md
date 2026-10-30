> Native migration scope: the web client is retired at the owner’s request.
> Web-specific implementation references below are historical; the behavior
> reference is `93f43ec:apps/web/...` (private pre-squash archive). Current clients are Linux and iPhone;
> their API gateway is `crates/zay-gateway`. Authentication ownership remains as
> documented here. See `docs/native/DECISIONS.md` for current decisions.

# ZayMusic — UX & bug-fix plan (2026-10-08)

Audit of the issues the owner reported plus the ones found while reading the code. Each item lists the root cause (with file references), the fix, and how to verify it. Phases are ordered by value per effort: Phase 1 is small and fixes things that are broken right now.

Ground rules for the implementer:

- The working tree already has uncommitted WIP in `settings/+page.svelte`, `library/downloads/+page.svelte`, `api/types.ts`, `zayfetch/main.py`, `tests/test_session.py`, and a new `e2e/session-recovery.spec.ts`. Build on top of it. Don't revert it.
- UI direction stays the same: when in doubt, copy Spotify.
- After every phase: `pnpm typecheck && pnpm test`, `pytest` in `services/zayfetch`, `pnpm test:e2e`, then rebuild the Docker app (`docker compose -f docker-compose.yml -f docker-compose.dev.yml --env-file dev.env up -d --build zaymusic`, plus `zayfetch` when the backend changed). Check `/healthz` and take screenshots at 390×844 (iPhone) and 1440×900.

---

## Phase 1: Broken behaviour (small fixes)

### 1.1 Music starts playing on app launch
**Cause:** `restore()` (`lib/player/engine.ts:552`) loads the saved track without playing it. That's correct, but loading a YouTube preview URL takes about 6.5 s because yt-dlp has to resolve it first. The browser fires `stalled` during that wait. `stalled` and `error` both go to `onError()` (`engine.ts:178`, `:186`). The guard there is `!playing && !track`, and it passes because a track is loaded, so the engine calls `audio.play()` 500 ms later. Installed PWAs and desktop browsers allow autoplay, so the music starts.

**Fix:**
- Add a private `wantsPlayback` flag. Set it in `play()`/`playList()`, clear it in `pause()`, and leave it false after `restore()`. In `onError()`, only retry when `wantsPlayback` is true.
- Stop treating `stalled` as an error: it just means the network is slow. Only retry on `error`.
- In `restore()`, set `audio.preload = 'none'` for the restored track so app launch doesn't start a yt-dlp extraction. Set it back to `'auto'` on the first `play()`.

**Verify:** Play a YouTube search result, reload the app, wait 15 s, and confirm it stays paused. Add a vitest case in which `stalled`/`error` fire after `restore()` and `play` is never called.

### 1.2 No previous button in the mini player
**Fix:** In `MiniPlayer.svelte`, add a `previous` button before play/pause (order: prev, play, next), using the same size as the other buttons. The buttons should get smaller rather than squeeze the title on narrow screens; see 4.4.

### 1.3 Scrollbar on Now Playing
**Cause:** `.sheet` uses `overflow: auto` (`NowPlaying.svelte:234`). On a phone, the 360 px art, controls, extras row, and volume row are taller than the viewport.

**Fix:**
- Make the sheet a fixed-height flex column with `overflow: hidden`.
- Change the art wrapper to `flex: 1 1 auto; min-height: 0`, and size the art to `min(100%, available height)` so it shrinks to fit instead of pushing content off screen.
- Remove the volume row on touch devices (`@media (hover: none)`). iOS ignores `audio.volume`, so that slider does nothing there.

**Verify:** No scrollbar or overflow at 390×664 (iPhone SE height), 390×844, or 430×932.

### 1.4 "· Radio" appended to autoplay songs
**Cause:** `QueueDrawer.svelte:83` concatenates the suffix onto the title. Also, `candidateToSong` (`stores/player.ts:36`) sets `viaRadio: true` on every candidate, so songs played from Recommendations are labelled too.

**Fix:**
- Remove the suffix.
- Add a parameter to `candidateToSong` so that only the radio provider sets `viaRadio`.
- In the queue drawer, split upcoming tracks into "Next up" and a separate "Autoplay" section with a one-line hint ("Similar songs based on what you're playing"), as Spotify does.

### 1.5 Imported playlists stop at ~200 songs
**Cause:** `importer.get_playlist_tracks()` calls `ytmusic_call("get_playlist", playlist_id)` (`importer.py:196`). In ytmusicapi 1.12.3 the signature is `get_playlist(playlistId, limit=100, …)`, so it stops after the first one or two continuation pages.

**Fix:**
- Pass `limit=None`.
- The `ytmusic_call` helper silently retries anonymously when the authenticated call fails. For a private playlist that returns an error or an empty result instead of the real tracks. For `get_playlist`, refuse the anonymous fallback when the playlist belongs to the account, and raise the "reconnect" error instead.

**Verify:** A pytest case with a stubbed client asserts `limit=None`. Live check: import a 500+ song playlist and compare `total` with the YTM count.

### 1.6 Playlist sync deletes your songs, and Settings offers "Remove" instead of "Pull"
**Cause:** `_sync_playlist()` (`importer.py:446`) rebuilds the Navidrome playlist from the YTM-resolved items only, deleting and recreating it. The reconcile loop calls it every tick while downloads are pending. As a result:
- Songs you add to an imported playlist get wiped.
- The playlist ID changes, which breaks links and on-device downloads.
- Your own ordering is lost.

**Fix (additive pull):**
- Backend:
  - Replace `_sync_playlist` with an append-only version. Read the playlist's current song ids, and for each resolved item whose song isn't in it, call `updatePlaylist songIdToAdd`.
  - Never delete, reorder, or recreate the playlist.
  - New YTM tracks are appended at the end.
  - Add `POST /youtube/imports/{id}/pull`. It re-reads YTM, seeds only new `videoId`s, and syncs.
  - If the Navidrome playlist no longer exists (deleted from the library), mark the import as unlinked. The next pull recreates the playlist instead of failing.
- Decided: a pull blindly re-adds any YTM song missing from the local playlist, including songs you removed locally.
- Settings UI (`YouTubeImport.svelte:169`):
  - Remove the "Remove" button. Each imported playlist gets a "Pull new songs" button, shown at all times.
  - The status line shows `N new · M downloading`.
  - Deleting the playlist is done from the playlist page in the Library, as with any other playlist.
- Keep `DELETE /youtube/imports/{id}` for internal use only, and call it from the playlist-delete flow so imports don't become orphaned.

**Verify:** pytest cases:
- Pull on a playlist that contains a local-only song keeps that song.
- The playlist ID is unchanged after a sync.
- A locally removed song is re-added by the next pull.
- New YTM tracks are appended.

### 1.7 Liked songs are never loaded
**Cause:** `loadLiked()` (`stores/library.ts`) is never called. After a reload every heart appears empty, and "unlike" on an already-liked song stars it again.

**Fix:** Call it in the root layout's startup sequence, next to `refreshDownloads()`. A Liked Songs page is covered in 3.2.

---

## Phase 2: Songs load slowly

**Measured on the running stack:**

| Request | First byte |
|---|---|
| Library stream (`/rest/stream`) | 0.005 s |
| YouTube track, URL cached | 0.10 s |
| YouTube track, first request | 6.5 s |

The delay only affects songs that aren't in your library: search results, autoplay/radio, and recommendations.

**Causes (`services/zayfetch/zayfetch/main.py:1070-1180`):**
1. `_preview_lock` is a single global lock around the whole extraction. Extractions run one at a time, so a skip waits behind the preload of the next track, any radio warm-up, and so on. This can add up to 10 s or more.
2. Each extraction starts a cold `yt-dlp` process (Python start, Deno challenge solver, PO token round trip).
3. The URL cache lasts 240 s (`+ 240`), but googlevideo URLs stay valid for about 6 h. The client preloads the next track while the current one starts. On a track longer than 4 minutes, the cached URL has expired by the time it's needed, so the preload is wasted and extraction runs again.
4. `trySwapPreload()` (`engine.ts:302`) copies the preload element's *URL* into the main element instead of swapping the elements. The bytes that were already buffered are thrown away and the stream is requested again.
5. When the cookie attempt fails, the cookieless retry runs afterwards instead of at the same time.

**Fix, in order:**
1. Replace the global lock with a per-video in-flight map (`dict[str, Future]`): concurrent requests for the same video share one extraction, and different videos run in parallel. Cap the number of parallel extractions at about 3 with a semaphore.
2. Set the cache TTL from the URL's `expire=` query parameter minus 10 minutes, with a 5 h fallback. Raise the LRU size from 256 to 1024.
3. Add `POST /preview/warm {videoIds:[…]}`: a fire-and-forget extraction for the next 2 queue items. Call it from `preloadNext()`, and from `ensureRadio()` when radio tracks arrive.
4. On the client, use two audio elements and swap them when the track advances, so the next track's buffered audio actually gets used. Keep the MediaSession, ReplayGain, and listener binding on whichever element is active.
5. Run the yt-dlp work in-process with a long-lived `yt_dlp.YoutubeDL`, reusing the player-JS and signature caches across calls. This is likely the biggest single gain, but it touches the PO-token plugin setup. Do it last, and only after steps 1-4 have been measured. Fall back to the subprocess on any error.
6. Log `extract_ms` per request so the improvement can be measured.

**Target:** next/skip within a queue under 0.5 s (warmed). A cold search-result play under 3 s.

**Done (2026-10-08).** Corrections from implementing it:
- Cause 1 was wrong: `_preview_lock` wrapped a helper nothing called, so extractions were never serialized. They were also never shared, so the player and the preload each ran their own extraction of the same video.
- The real cost is YouTube round trips, not process start-up: the watch page (~1.3 s), the web_music client config (~1.2 s), the player API, and the Deno challenge solve. Running in-process saves ~1 s and skipping the client-config request (`player_skip=configs`) saves ~1 s more. Skipping the watch page breaks signed-in extraction.
- Step 4 (swapping audio elements) was not done. iOS only lets the element that is already playing keep going in the background, so a swap risks stopping playback on the lock screen. Warmed URLs make the main element's request fast anyway (0.25-0.34 s). Preview tracks are no longer buffered in the preload element.

**Result:** warmed next/skip 0.25-0.34 s (target met). Cold play 4.3-5.2 s (was 6.5 s; target of 3 s not met). Signed-out extraction takes ~1.7-3.5 s but returns 128 kbps (itag 140) instead of Premium's 256 kbps (itag 141), so signed-in stays first.

---

## Phase 3: Albums out of the spotlight, and a Home page about your listening

Mental model: **an album is an official playlist you can't edit. It's in your library only if you saved it.** Navidrome creates an album for every downloaded file, so "every album in Navidrome" is not the same as "your albums".

### 3.1 Saved = starred
- Library → Albums shows only starred albums (`getStarred2().albums`), not `getAlbumList2('alphabeticalByName')`.
- On the album page, replace the heart with a Spotify-style save control: + in a circle, which turns into a filled green ✓. It stars or unstars the album and is labelled "Save to library" / "Remove from library".
- When you save a YouTube Music album from Search, zayfetch records an `album_save` intent. Once the tracks are indexed, the reconcile loop stars the matching Navidrome album, matched by album and artist tags.
- Downloading a single song never adds its album to the Library view.

### 3.2 The Library page (Spotify-like)
- Default view is a single list sorted by recently played: **Liked Songs** pinned first, then playlists, then saved albums. The filter chips are Playlists / Albums / Artists, with Playlists first. The default is no longer `'albums'` (`library/+page.svelte:24`).
- New route `/library/liked` uses the existing `TrackList`, the `likedSongs` store, play/shuffle, and the download-all button.
- Replace the custom `<select>` sort with a styled sort menu (Recently played / Recently added / A-Z).
- Performance: `sortPlaylistsByLastPlayed` currently calls `getPlaylist` once per playlist, every time the Library opens. Instead, record a `{contextType, contextId, playedAt}` entry when playback starts from a playlist or album (see 3.3) and sort by that.
- Desktop sidebar: remove the "Your collection" block, which starts with Albums and duplicates the Library nav item. Replace it with a Spotify-style "Your Library" list: Liked Songs, then playlists, each with a cover thumbnail.

### 3.3 Home page
Current sections:
- "Jump back in": recently played albums.
- "Recently played": recently played songs.
- "Recently added": an album grid.

The first two overlap, and all three are about albums.

New layout:
1. **Greeting, then a shortcut grid** (Spotify's 2×4 tiles) of recent *contexts*: Liked Songs, playlists, and saved albums you played from. This replaces both "Jump back in" and "Recently played". It needs `playList(songs, i, context?)` to record a context (in Dexie `recents`, or a new `contexts` table).
2. **Your top songs:** the most-played songs from Navidrome play counts, about 10 in a horizontal row or a compact 5-row list with "Show all".
3. **Rediscover:** songs with play count ≥ 3 that you haven't played in more than 30 days. Show 6-8, picked with a date-seeded shuffle so the set changes daily. Hide the section when fewer than 4 songs qualify.
4. **Recommended for you:** the existing `Recommendations`.

Remove the album grids.

Backend: add `GET /home` in zayfetch. It pages through Navidrome `search3` (empty query) server-side, reads `playCount` and `played`, and returns `{topSongs, rediscover}`. The phone doesn't pull the whole library. Cache it for 10 minutes.

**Done (2026-10-08).** Changes from the plan:
- Rediscover qualifies songs played at least twice *or* liked, and not played in 21 days (the plan said ≥3 plays and 30 days). With about two weeks of play history neither version has enough songs yet, so the section is hidden for now.
- The Library is a list, not a grid, on every screen size, like Spotify's library. The filter chips toggle: tapping the active chip clears it. Sorts: Recents (the later of last played and saved/created), Recently added, Alphabetical.
- Saving a YouTube Music album is the "Save album" button in the Search album view. It queues the downloads and calls `POST /album-saves`. An album that is already indexed is starred immediately; otherwise the reconcile loop stars it once indexed, and gives up after 3 days.

---

## Phase 4: One design system

### 4.1 Icon library
`Icon.svelte` draws 35 icons by hand as SVG paths, and they're uneven. Replace the internals with **Phosphor** (`phosphor-svelte`, Svelte 5 compatible). Phosphor has regular and *fill* weights, which gives Spotify's filled active states: a filled home or library tab icon when that tab is selected, a filled heart, filled play/pause.

Keep `<Icon name size active>` as the only API: a name → component map, with `active` selecting the fill weight. No call sites change. (Open question 3.)

### 4.2 Design tokens and leftover colours
Some colours from an older warm/orange theme are still there:
- Dark-brown text on the green accent: `#20140f`, `#24150f`, `#1c100f`.
- An orange header gradient on the album and playlist pages: `rgba(227,130,89,.14)`.
- Brown backgrounds: Now Playing gradient `#2b2620`, grabber `#4c493f`, scrollbar `#4b4940`, player bar `rgba(28,28,25,.96)`.

Fix:
- Add tokens to `app.css`: `--on-accent`, `--surface-1/2/3`, `--hover`, `--radius-sm/md/lg`, a type scale (`--fs-xs…--fs-2xl`), and spacing.
- Replace every hard-coded colour.
- Generate the playlist and album header gradients from the cover art's average colour, as Spotify does, falling back to neutral grey.

### 4.3 Shared components (replacing unstyled native controls)
- `IconButton`: sm/md/lg sizes, used for all round icon buttons. There are currently about 8 separate implementations (`.t`, `.hdr-btn`, `.icon`, `.icon-btn`, `.mode`, `.save`, `.vol-btn`, `.close`).
- `SaveButton`: the single + / ✓ control, used everywhere a track or album can be saved:
  - For a YouTube preview it queues the download.
  - For a library song it toggles Liked.
  - The Now Playing "Add to library" pill (`NowPlaying.svelte:190`) becomes this button, placed where the heart is now.
- `Switch`: replaces the native checkbox for queue-drawer Autoplay and the Omnibar multi-select.
- `ProgressBar`: replaces native `<progress>` (imports, downloads).
- `Menu` / `BottomSheet`: on mobile the track ⋯ menu and the add-to-playlist picker open as a bottom sheet; on desktop they stay popovers. Also used for the Library sort.
- Style Settings' `<details>`, `<textarea>`, and file input with the same tokens.

### 4.4 Truncated text
Truncation is mostly caused by layout crowding.

**Track rows:**
- Currently, on touch devices, every row shows heart, ⋯, + and the duration (`TrackList.svelte` actions). That leaves about 150 px for the title on a phone.
- Mobile: title and artist, then a single ⋯ that opens a bottom sheet with Like / Add to playlist / Download / Play next / Go to artist / Edit / Delete. Show a small green ✓ only when the song is liked. No duration.
- Desktop: Spotify columns (# · title/artist · album · ♥ on hover · duration · ⋯ on hover).

**Cards:** Album, playlist, and artist names wrap to 2 lines (`-webkit-line-clamp: 2`) instead of being cut to one.

**Now Playing title:** If it overflows, scroll it marquee-style, as Spotify and Apple Music do. Put artist and album on separate lines.

**Mini player:** The title uses the full row width, and the three control buttons are 36 px.

**Then:** At 390 px, go through every route and remove any ellipsis that cuts a word that would fit. Record screenshots in the PR.

### 4.5 Smaller issues found in the audit
- `playList()` awaits an IndexedDB lookup before calling `play()`. On iOS that sometimes loses the user-gesture permission, which is why "Tap to resume" appears. Call `audio.play()` synchronously on the gesture (a silent unlock), then switch the `src`.
- `TrackList` renders every row. A 500+ song imported playlist means more than 2,000 buttons on an iPhone. Virtualize the list (this was deferred in NOTES.md and is needed now).
- The Downloads button in the Library header takes as much visual weight as the page itself. Move it into the Library's ⋯ menu or a small icon.

**Done (2026-10-08).** Changes from the plan:
- Icons use Phosphor's *bold* weight by default (regular looked thin at 16-20 px). `active` selects *fill*, and transport icons are always filled. The Library tab uses `Books`, and the selected tab is filled.
- Switch, ProgressBar, and the styled `<details>`/`<textarea>`/file input are global CSS on the native elements (`input[role=switch]`, `progress`, …) in `app.css`, not wrapper components. They keep native semantics and need no wiring.
- Not done: a shared `IconButton` to replace the 8 round-button implementations. They now share tokens, but the classes still exist.
- Virtualization is batch rendering: 120 rows at a time, more as a sentinel scrolls into view, and the playing row is always rendered. This is not true windowing. A 1,148-song playlist opens with 120 rows of 2 buttons each on a phone.
- The iOS fix "unlocks" the element by calling `play()` synchronously inside the tap (muted; a silent WAV when there's no `src`) before the awaits in `playList()`. Event listeners ignore the silent clip. It has not been tested on a real iPhone yet.

---

## Decisions (2026-10-08)
1. **Pull after you remove a song:** the next pull blindly re-adds it.
2. **Artists tab:** only artists you follow (starred), with a Follow button on artist pages.
3. **Icons:** Phosphor.

## Order of work
Phase 1 (one PR) → Phase 2 (backend PR, then client PR) → Phase 3 (one PR) → Phase 4 (4.1 + 4.2 + 4.3 together, then 4.4 + 4.5). Each PR is rebuilt and checked on the Docker app at `:3847` before it's reported done.
