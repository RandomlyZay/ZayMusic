# Current scope

The owner's later instruction retires and removes the web client, overriding the
original fallback requirement. The feature contract remains its historical source
at `93f43ec:apps/web` plus the ported Rust regressions. Earlier decisions below
are historical where they describe a supported web client or hosted Linux CI.

- Use Rust for the shared native core and HTTP credential gateway. Keep Python for
  ZayFetch's yt-dlp, ytmusicapi, tagging and managed-browser integration: a wholesale
  rewrite would delay native parity while still needing those upstream tools.
  Revisit further extraction after device validation, with a measured benefit.
- Replace SvelteKit's server proxy with `crates/zay-gateway`, preserving `/rest`
  and `/api/fetch`, streaming/ranges, server-owned credentials and the native server
  URL. Do not migrate Google authentication ownership or production deployment.
  The old dev web image is retained as `zaymusic-zaymusic:web-93f43ec-backup`.
- The gateway accepts native requests without Origin, rejects mismatched/cross-site
  browser origins, preserves repeated query/form fields, strips client auth fields
  and injects server auth. It forwards compressed bytes and lengths consistently,
  follows only same-origin redirects, and never logs upstream credentials.
- Report the final Linux track at end-of-stream as well as handoffs and clocks;
  a short final track can finish before the next timer tick. The real silent-WAV
  regression checks all three history entries and per-track gain.
- Native GTK fixture drivers locate controls by label and post-layout geometry;
  actions still use real mouse input. Test markers omit editor/session text.
- Collection Play shuffles playlists and Liked Songs; albums remain ordered.
  Track taps preserve the current shuffle mode except inside albums. Previous
  restarts the current track beyond three seconds. These rules live in Rust.
- Persist playback position with the play epoch and song ID, reject stale clocks,
  clear it on actual track changes, and restore paused. Engines report real clocks.
- Cache offline library pages and media metadata, but never Settings/session/device
  codes. Rebuild Home history and recent shortcuts from local state when offline.
- A failed acquisition retains the full request, including playlist targets, for
  retry. Queue album tracks in order before saving the album; don't claim a save
  when any queue request failed. Native offline downloads require a library song;
  acquire previews first so both apps have the same ownership/lifecycle.
- Search ranking and pagination are shared. Fetch independent 40-item Subsonic
  pages until exhausted, deduplicate buggy repeated pages, then let native filters
  operate over stable section IDs. Empty search opens local listening history.
- A maximum of eight Home shortcuts is filled from play contexts, Liked Songs and
  playlists. Pinned Liked Songs, library filters and sorting are shared catalog rules.

---

# Native migration decisions

- Keep the existing Tailscale-only SvelteKit `/rest` and `/api/fetch` gateway as the native clients' server API. It already injects credentials on the server, accepts native HTTP clients, streams ranges, and is reachable at the existing HTTPS origin. Native clients need only the server URL; distributing server or Google credentials is unnecessary. PLAN-AUTH's managed browser remains the sole owner of Google sessions. No production changes are needed.
- Use UniFFI proc-macro exports with JSON command/result envelopes at the boundary. Rust owns typed models, queue mutations and SQLite. This keeps wire compatibility with the web API while allowing incremental native UI implementation. Audio remains platform owned.
- Preserve the web fallback and its behavior. The owner’s later dependency-update request authorizes its SvelteKit 3 migration and development deployment; production remains untouched. Native installation remains manual, with no update channel.
- Treat unimplemented features as pending in PARITY.md. They are not impossible natively; claiming completion without exercised behavior would hide defects.

- Package the UniFFI C module exclusively through the XCFramework's Headers directory. Adding the generated binding directory to both Swift and Clang include paths defined `zay_coreFFI` twice in the first macOS build. Keep generated Swift sources separate from packaged C headers. Include both arm64 and x86_64 simulator slices because GitHub runner architecture can differ.
- Make bindgen a host-only Cargo feature to avoid compiling the binding generator and its CLI into every mobile target.
- Expand SwiftUI view closures rather than compressing whole screens onto a line. The second iOS run exposed an unclosed sheet closure after packaging succeeded; separate computed views make structural mistakes easier to detect in review.

- Use the managed Fedora Swift compiler on devbox to parse iOS sources and compile/run generated UniFFI bindings against the Linux core. CI repeats this on an official Swift Linux image and gates the macOS job on success, conserving macOS minutes. Apple-framework behavior still requires the simulator and devices.
- Retain the prepared Rust queue states until advancement consumes them, invalidating them on queue mutations. Generating a fresh random repeat cycle for each snapshot could make the preloaded native audio disagree with the queue's actual next track. Snapshots are read-only in SQLite.
- Install the Swift Linux container's build prerequisites explicitly before the Rust toolchain action. The official `swift:6.0` image does not include `curl`; assuming runner-host utilities existed inside the container prevented the bridge check from starting. Include Python and C build tools for the binding generation and bundled SQLite build as well.

- Rust now supplies the same browse/collection view models, cached offline pages, URL classification, preview metadata and explicit acquisition behavior to both UIs. Per-platform catalog switches were duplicated business logic and could drift. Album saves/follows still use the existing gateway's starred collection semantics.
- Screenshot review of the first green iOS run found hidden tab controls and stale/blank captures. Keep the mini player inside each tab's safe area and require selected, hittable tabs plus screen-specific content before snapshot capture. Export attachments even if a later simulator assertion fails.
- Headless GTK navigation checks wait for a test-only page-ready marker after rendering rather than a fixed one-second delay. The server status request can take longer; a fixed sleep captured the preceding queue screen under a Settings heading. The marker contains only the page name and loading flag.

- Autoplay coordination lives in Rust: share an in-flight request, prefetch only near exhaustion, suppress it for repeat/woven queues, discard responses after a new queue or autoplay toggle, and deduplicate against main/priority/active recordings. A separate autoplay epoch invalidates radio work without cancelling an unrelated pending shuffle.
- A woven queue uses its constructed order directly. Shuffling the woven result again destroyed the specified gaps. Persist its density so restored woven queues keep suppressing additional autoplay.
- GStreamer `about-to-finish` prepares the next URI; the shared queue advances on the next stream-start message. Decoder completion can precede the audible boundary. Keep a pending-URI sequence for short buffered tracks and apply each track's ReplayGain at stream start.
- Native handoff tests generate temporary silent WAV files, outside the library and outside committed assets. They exercise real GStreamer/AVQueuePlayer sequencing without seeding the development library or depending on real credentials. They establish order and control behavior, not audible gap duration or LiveContainer background reliability.

- All transfers use monotonically allocated SQLite tickets. Progress and completion are accepted only for the current active ticket, so cancel/delete/retry cannot resurrect an old download. Completion copies to an app-owned incoming file, syncs it, then atomically renames it before marking the catalog ready. Existing ready downloads migrate in place.
- Desktop bulk downloads persist the whole batch before two workers start; cancellation is polled by a cancellable async HTTP transfer even while the server stalls. SQLite is authoritative for both desktop and URLSession progress, failure, storage totals and retry.
- iOS background sessions are recreated by a shared environment and UIApplicationDelegate on background launch. Reconcile OS-owned tasks before accepting new work and invoke the system completion handler after background events finish. Delegate diagnostics are fixed messages, never raw request URLs or exceptions. LiveContainer's handling still requires a device test.
- The native playback fixture now registers its temporary WAV files in the actual offline catalog. Both native playback checks therefore use native offline files with an unreachable server, rather than bypassing the catalog with preview file URLs.

- Keep playlist mutations in Rust, serialized per core instance. Re-read the server playlist before index-based removal/reorder and check the selected recording still occupies that row. Reorder replaces entries in the existing playlist rather than creating a second playlist. Preview songs enqueue acquisition with the target playlist ID.
- Send OpenSubsonic mutation parameters in an encoded POST form, preserving repeated song IDs, and supply the configured server origin for CSRF validation. The existing gateway collapses repeated query parameters. Live mutation checks use the existing Tailscale HTTPS gateway; do not weaken CSRF or pin the development gateway to a single HTTP origin.
- Separate a collection's route from its display title. A playlist named Downloads or Queue must not activate the corresponding system screen's refresh logic. Shared collection results include context and playlist positions; platform clients only render and invoke the shared commands.
- Use an isolated, loopback-only Python fixture server for native playlist UI tests and the Linux Swift bridge. This exercises actual Swift/Rust/HTTP mutations without real credentials or account data. Screen tests wait for the reordered track's accessible position before the next edit.
- Present the playlist picker with an identifiable selected-song item rather than separate Boolean and payload state. The first simulator editing run reached the picker but failed its mutation and left the sheet open; item-based presentation binds the selected recording to that presentation. Show the selected title and assert it before editing. Stop the UI test at the first failure to avoid misleading tab-occlusion errors after a failed sheet action.
- Use a standard 44-point toolbar Button to present collection actions in a native bottom sheet. The simulator still reported the SwiftUI toolbar Menu as non-hittable after its accessibility label moved to the control. Keep the hittability assertion; expose the actions through the familiar bottom-sheet layout. Run selected actions after dismissal so editors and destructive confirmations have a clear presentation boundary.

- Use native platform components wherever they fit (owner's explicit design constraint). The redesign styles stock GTK4/libadwaita and SwiftUI lists, navigation, sheets, popover/context menus, dialogs, sliders, switches, toasts and empty states rather than drawing imitation widgets or replacing working gestures. Visual identity comes from artwork, spacing, type and a restrained green accent. DESIGN.md is the layout reference.
- Resolve and cache artwork in Rust. Both clients call `artwork(item, size)`, which prefers a preview's thumbnail and otherwise requests the gateway's `getCoverArt` with only the server origin, keeps a bounded on-disk cache under the app data directory, and remembers misses for the session. One rule keeps art identical across clients and available offline; platform image decoders still own display. Sizes are bucketed to limit cache variants.
- Linux decodes artwork on four dedicated worker threads and drops page-scoped requests after navigation. GIO's shared task pool also serves GTK's own asynchronous I/O; hundreds of blocking HTTP fetches there could stall it.
- Placeholders are deterministic gradients with a kind glyph, using one palette on both platforms. Grey boxes with a music note read as missing content; a stable per-item colour reads as intentional and keeps rows recognisable before art arrives.
- Headers show only server facts. Home sections come from the existing `/home` response (`topSongs`, `rediscover`) as section metadata in the shared view model; the greeting is local time of day. Recent contexts remain pending rather than simulated.
- Replace deprecated `GtkDialog`/`GtkMessageDialog` with `AdwAlertDialog`/`AdwDialog` (libadwaita 1.5, matching Ubuntu 24.04 CI). They are presented inside the main window, so headless checks type into the focused entry and use window coordinates instead of searching for separate toplevels. Coordinates were re-measured from captured screenshots; every fixture state assertion is unchanged.
- Set libadwaita's accent colour to the brand green so suggested buttons, focus rings and switches agree, instead of restyling individual buttons. iOS uses a white global tint: SwiftUI's default button style tinted entire list rows, which produced the earlier green subtitles.
- Add a Linux seek bar using the same flushing GStreamer seek as MPRIS, and an iPhone Now Playing slider using `AVQueuePlayer.seek`. Volume remains pending. The lock screen now receives artist, album, duration and artwork.
- The fixture server generates striped "disc" PNG covers so screenshots exercise real artwork loading without account data. They are deliberately unlike the app's placeholders.

- Preserve iPhone catalog snapshots per tab while keeping one playback engine and core. Switching tabs invalidates the old tab's in-flight catalog work; returning restores its collection, rows and title. This prevents a playlist hero/header from leaking into Home or Search. Simulator navigation regression checks accompany the change.
- Load a native iPhone catalog destination on its first appearance rather than on every tab reappearance. SwiftUI reruns `.task` when returning to an existing destination; reloading its original list overwrote the restored open collection. A view-local lifecycle guard preserves restored state, and a cancelled initial visit is allowed to retry. Search follows the same lifecycle; tab changes no longer issue a duplicate initial load.

- Adopt Apple’s native Liquid Glass on iOS 26/27 by linking a modern SDK and using the system tab-bar bottom accessory for the mini player. The previous CI default was Xcode 16.4/iOS 18.5, which retained legacy system chrome. Use the newest stable preinstalled Xcode and modern simulator on macOS 26 runners, gate SDK/app metadata, and keep a native material inset only for earlier OS releases. Do not imitate glass with custom blur layers.
- The owner requested every dependency and CI action be updated. Use current stable project libraries, refreshed compatible transitives, exact action commits, current supported runtime images and locked Python installs. Node uses the newest LTS line; Apple builds use supported stable installed toolchains rather than downloading beta SDKs or spending the macOS budget on large toolchain installs.
- Use TypeScript 6 and 7 side by side, as explicitly authorized: SvelteKit/svelte-check retain their supported TS6 peer; a package alias runs TS7 over the project separately. Both checks are required.
- Generate the web worker through a wrapper around the Node adapter, before its asset table is hashed. The generic Vite PWA HTML hooks no longer fit SvelteKit 3. Direct Workbox generation preserves explicit registration, actual cached assets, offline shell fallback and network-first online deep links. Dedicated browser contexts test real workers; mock-based tests block them to preserve request interception.
- Run dependency tests locally on devbox ahead of macOS; the owner’s later budget request removes hosted Linux jobs. The web browser harness and service tests use fake credentials and loopback fixture data, never real server sessions. Docker contexts exclude secrets, local profiles, development data and generated build caches.

- Derive `IPHONEOS_DEPLOYMENT_TARGET` from the XcodeGen project before Cargo builds, so Rust and native C dependencies use the app’s existing iOS 17 minimum on every device/simulator slice. This is a minimum OS, separate from the modern SDK used for Liquid Glass. Run 40918440196 exposed Rust’s unset default of iOS 10 against C objects built for the SDK version, failing the AWS-LC stack-probe link. Include the SDK and Apple build configuration/scripts in the Rust cache key. Verify Rust’s effective minimum on Linux and before each Apple slice build. [Rust’s iOS target documentation](https://doc.rust-lang.org/rustc/platform-support/apple-ios.html) documents this environment variable.

- GitHub Actions is macOS-only; the devbox local-check script replaces the Linux jobs. At the owner’s request, PR events build/test automatically so checks are attached to the PR for watcher notifications. Manual dispatch remains available outside PRs. Concurrency is scoped per PR/ref. Keep existing downloadable IPAs outside artifact expiry; use 14-day IPA and 7-day simulator artifact retention. No subscription or billing setting is changed.
- Listening history and scrobble thresholds belong in the shared core. Native engines report actual clocks/completion with a shared playback epoch; queued lookahead, paused/previews and stale callbacks never count. Record up to 50 recent library songs locally and submit online scrobbles once per play at half the duration or four minutes, following the web engine. Repeat-one starts a fresh epoch so a genuine new play counts. SQLite/HTTP work runs outside audio/UI callbacks; transport failures remain best-effort, as on the web, rather than inventing an offline scrobble queue.

- Manual iOS runs are attached to PR #4 in GitHub, but T3 missed their failure
  notifications after announcing the earlier passing check. Do not treat a watcher
  summary as the workflow result: verify the run conclusion before claiming green.
  This is an observed notification failure; no T3 production changes were made.
- SwiftUI Lists/Forms materialize rows lazily. UI tests reveal controls before
  querying state, and inspect album search results through the native Albums filter
  instead of requiring off-screen rows to exist. Product assertions remain intact.

- Native tag edits use the additive ZayFetch `/music/{id}/tags` endpoint rather than
  depending on a server-specific Subsonic update extension. Only six allowed fields
  can change. Omitted fields stay untouched; null/empty clears optional fields,
  and title cannot be cleared. Write and verify a sibling copy before atomic replace,
  preserving audio, ReplayGain, artwork and unrelated tags. Request a scan afterward.
  Unsupported containers return 415 without changing the file; test-device metadata
  edits should use a disposable track first. Python's established Mutagen integration
  remains the implementation, consistent with retaining acquisition in Python.

- Count the Jobs badge in Rust (`attention_summary`): failed downloads, unsent
  acquisition requests and failed server jobs are disjoint, so they are summed.
  Clients re-read server failures at launch, about once a minute and after management
  actions; previously server failures only appeared after Jobs was opened. An
  unreachable server keeps the last known count instead of hiding failures. When
  the Needs attention filter empties (e.g. after Retry), Linux shows all jobs.
- Keep previous/next track buttons on the iOS lock screen and support scrubbing,
  instead of the web client's ±10 s seek actions: iOS shows skip-interval buttons in
  place of track buttons, which breaks the Spotify/music-app convention.
- Parity was re-audited against `93f43ec:apps/web`. As in the web client, search
  suggestions appear only for empty results, a failed track pauses with a message
  rather than skipping, and failed imports link to the failure list rather than
  per-track pages. These are faithful behavior, not native gaps.
- The repository is public at the owner's request, after a history/log/PR audit
  (gitleaks over all refs, Actions logs and PR text; no secrets or real account IDs).
  Tailnet hostnames remain in code defaults and history; they are not credentials.
  The README states it is a personal project with no license; the MIT Cargo metadata
  was removed. Issues, wiki and projects are disabled. Standard macOS runners are
  free for public repositories. PR checks run automatically; batch related changes.
- At the owner's request the public history was squashed into one commit on `main`.
  A force-push alone leaves old commits reachable through PR refs and SHA URLs, so the
  original repository was made private and renamed as an archive, and a new public
  `ZayMusic` received only the squashed tree. The archive keeps PR #4, CodeRabbit
  reviews and Actions history; a full local git bundle is kept on devbox outside the
  worktree. The native work lands directly on `main`; no draft PR remains.

- Bound `ContentView`'s generic metadata with `AnyView` at each tab and between
  groups of root presentation modifiers. The iPhone/LiveContainer startup report
  for the retained 98ab134 IPA (UUID e441e14f-0625-39b7-84f4-4ac70f067013)
  hit a main-thread stack guard during Swift type decoding before the first frame.
  Its app offset 133044 falls inside `ContentView.body` (symbol starts at 131920);
  offset 128256 is the concrete-type metadata instantiator. Splitting computed
  `some View` properties alone still exposes their underlying generic types.
  Keep tab tags, labels and badges outside erasure so TabView sees their traits.
  Linux parsing/bridge checks cannot establish device startup success; verify the
  rebuilt IPA in LiveContainer as well as simulator UI tests.
- Device follow-up showed root type erasure alone was insufficient: the b9ace78
  IPA UUID 05edda70-4f67-3a85-9f29-716fb55498e9 crashed in `___chkstk_darwin`
  while constructing `libraryList` (app offset 198004), through the eager tab
  and presentation factories. The device report gives a 1008 KB main stack.
  Use a concrete `ScreenContent` View to evaluate each screen factory in its
  own SwiftUI body update; returning AnyView from a computed property alone
  still constructs its whole payload on the caller's stack. Erase Library link
  and track-row payload types too, bounding the values stored inline in lists.
  Simulator success does not verify the device's smaller stack budget.
