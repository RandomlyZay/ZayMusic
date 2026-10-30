# Native client design

Both clients follow familiar Spotify structure — a library sidebar on Linux, tabs
and bottom sheets on iPhone — built from each platform's own components. This file
is the reference for layout, tokens and component choices; DECISIONS.md records why.

## Principles

1. **Native components first.** Use stock GTK4/libadwaita and SwiftUI/UIKit controls
   for lists, navigation, sheets, menus, dialogs, sliders, switches, toasts and
   empty states. Improve composition, artwork, hierarchy, spacing, colour and type
   around them. Do not draw imitation widgets or replace working platform behaviour
   (selection, keyboard focus, context menus, swipe actions, accessibility) with
   custom gesture-heavy controls.
2. **Artwork carries the identity.** Every song, album, artist and playlist row has a
   square tile (round for artists). Real cover art comes from the gateway through the
   shared core; until it arrives, or if none exists, a deliberate placeholder shows.
3. **Restrained green.** `#1ED760` marks only play buttons, the now-playing title,
   enabled switches/toggles, active shuffle/repeat and the "downloaded" indicator.
   Chrome and secondary text are white/grey. Subtitles are never tinted.
4. **Only real data.** Headers show facts from the server (owner, artist, year, song
   count, total length). No invented recents, activity, play counts or greetings
   other than the local time of day.

## Tokens

| Token | Value |
|---|---|
| Background | `#121212` content, `#000000` Linux sidebar; iOS system black |
| Elevated surface | `white 16%` (iOS mini player, search field) |
| Accent | `#1ED760` (hover `#3BE477`), on-accent text black |
| Secondary text | Linux `alpha(white, .62–.70)`; iOS `.secondary` |
| Type | System font (Adwaita Sans/Cantarell; SF Pro). Hero 44 px/800 on Linux, `.title2.bold` on iPhone; track title 600 weight/`.body`; meta caption/`.subheadline` |
| Art radius | 4 px rows, 8 px large tiles, circle for artists |
| Art sizes | Linux 32 sidebar, 40 track, 52 browse, 56 player, 192 hero. iPhone 40 mini player, 48 rows, 52 library, 220 hero, up to 360 Now Playing |

**Placeholders:** a diagonal gradient chosen from eight muted hue pairs by a hash of
the item name, with a white symbolic glyph for the kind (note, disc, person, list,
guitar). Liked Songs uses the familiar violet→mint gradient with a heart. Both
clients use the same palette.

## Linux (Relm4, GTK4, libadwaita 1.5+)

- `AdwApplicationWindow` → `AdwOverlaySplitView`. Sidebar: `AdwToolbarView` with a
  header and a `.navigation-sidebar` `GtkListBox`: Home, Search; *Your Library*
  (Liked Songs, Playlists, Albums, Artists, Downloads, Queue); *Browse* (New music,
  All Albums, All Artists, Settings); *Playlists* with each playlist's cover.
- Content: `AdwToolbarView` with an `AdwHeaderBar` holding a clamped
  `GtkSearchEntry` and a primary `GtkMenuButton` (Autoplay check item, Settings), an
  `AdwBanner` for offline mode, and an `AdwToastOverlay` for transient messages.
- Page header: collections and Liked Songs show a 192 px cover, kind label, large
  title and real meta. Other pages show a `title-1` heading and count; Home uses a
  time-of-day greeting.
- Action row: green circular Play, Shuffle, Fresh shuffle menu (sparse/dense),
  Download all, and a `view-more` popover menu for collection actions. Playlists
  shows a pill **New playlist** button. Queue shows an Autoplay `GtkSwitch`.
- Track rows (`GtkListBox`, activate to play): index or track number, 40 px art,
  title over artist (Preview badge for YouTube items), album column, download
  badge/progress, duration and a `GtkMenuButton` with a sectioned `GMenu` popover.
  Queue rows add up/down/remove icon buttons. Home and Queue are split into titled
  sections (Your top songs / Rediscover; Next in queue / Main queue).
- Browse rows: 52 px art, name, "Album · Artist · Year", "Playlist · N songs",
  "Artist · N albums", with a popover menu for downloads/rename/delete.
- Dialogs: `AdwAlertDialog` for create/rename (with an entry), delete and remove-all
  confirmations; `AdwDialog` with a boxed list for Add to playlist.
- Player bar (`GtkCenterBox`): 56 px art, title/artist; shuffle and repeat
  `GtkToggleButton`s, previous/next, white circular play/pause, elapsed and total
  labels around a seek `GtkScale`; a Queue toggle on the right.
- Empty and error states use `AdwStatusPage`; loading uses `GtkSpinner`. Settings is
  a `.boxed-list` with an `AdwSwitchRow` and informational `AdwActionRow`s.

## iPhone (SwiftUI, iOS 17)

- `TabView`: Home, Search, Library, Settings, each in its own `NavigationStack` with
  the mini player as a bottom `safeAreaInset` above the tab bar. Global tint is
  white; green only where listed above.
- Lists use `.plain` style without separators. Song rows: 48 pt art, title (green
  when playing), "PREVIEW" chip / downloaded glyph, artist; `.contextMenu` for every
  action and a leading swipe for Play next. Home and Queue use section headers.
- Collections and Liked Songs: inline navigation title plus a centred 220 pt cover,
  title, meta and a row with Download, Shuffle and a green circular Play.
- Library: plain list of destinations with icon tiles and one-line descriptions,
  grouped into Collections, Browse and ZayFetch.
- Search: segmented Library/New music picker and a rounded search field.
- Empty states use `ContentUnavailableView`; loading uses `ProgressView`.
- Mini player: rounded elevated bar with art, title/artist, play/pause and next
  (44 pt targets) and a thin progress line. It opens Now Playing.
- Now Playing: a native sheet with drag indicator: close, AirPlay route picker,
  large art, title/artist, a seek `Slider` with elapsed/remaining time, and
  shuffle/previous/play/next/repeat. The lock screen receives artist, album,
  duration and artwork.
- Collection actions remain a standard 44 pt toolbar button presenting a bottom
  sheet; Add to playlist remains an item-presented sheet showing the selected song.

## Accessibility and test hooks

Artwork is decorative (`accessibilityHidden` / `img` role with the item name on
Linux). Icon-only buttons have labels/tooltips. Stable iOS identifiers:
`collection-actions`, `playlist-selected-song`, `row-<id>` (value
"Playlist position N"), `mini-player`. Picker rows keep the playlist name as their
label and expose the song count as the value. Linux headless tests use the
`ZAY_UI_READY_FILE` marker and window-relative coordinates for the 1100×760 layout.
