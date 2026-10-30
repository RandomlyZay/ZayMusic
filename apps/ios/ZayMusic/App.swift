import SwiftUI
import UIKit
import AVFoundation
import MediaPlayer

/// MediaPlayer requests artwork off the main thread, so the handler must not be main-actor isolated.
func lockScreenArtwork(_ image: UIImage) -> MPMediaItemArtwork {
    MPMediaItemArtwork(boundsSize: image.size) { _ in image }
}

@MainActor final class Music: ObservableObject {
    @Published var rows: [[String: Any]] = []
    @Published var page = "Home"
    @Published var error = ""
    @Published var current = "Nothing playing"
    @Published var playing = false
    @Published var kind = "songs"
    @Published var headerTitle = "Home"
    @Published var collectionContext: [String: Any] = [:]
    @Published var notice = ""
    @Published var offline = false
    @Published var searchMode = "Omnibar"
    @Published var autoplay = true
    @Published var volume: Double = 1
    var searchQuery = ""
    @Published var currentSong: [String: Any] = [:]
    @Published var shuffleOn = false
    @Published var repeatMode = "off"
    @Published var sections: [[String: Any]] = []
    @Published var recentContexts: [[String: Any]] = []
    @Published var suggestions: [String] = []
    @Published var correction = ""
    /// Search chip over the core's mixed results: all, songs, albums or library.
    @Published var searchFilter = "all"
    /// Unfiltered search results; `rows` and `sections` hold what the chip shows.
    var searchSourceRows: [[String: Any]] = []
    var searchSourceSections: [[String: Any]] = []
    /// A resolved YouTube playlist or album link ("playlist"/"album") and its title.
    @Published var resolvedKind = ""
    @Published var resolvedTitle = ""
    @Published var loading = false
    /// Player problems belong to Now Playing, not to the tab's catalog.
    @Published var playbackError = ""
    @Published var radioError = ""
    @Published var jobFilter = "all"
    @Published var connectionWarning = ""
    /// Failed jobs and downloads, shown as the Library tab badge.
    @Published var attentionCount = 0
    /// Catalog snapshots of pages a collection was opened from, newest last, so Back
    /// returns to the exact search results, shelf or list (per tab, like Spotify).
    @Published var trail: [[String: Any]] = []
    let artCache = NSCache<NSString, UIImage>()
    var nowPlayingArt: UIImage?
    var wantsPlayback = false
    var resumeAfterRadio = false
    var radioTask: Task<Void, Never>?
    var timeObserver: Any?
    var playbackEpoch: UInt64 = 0
    @Published var storageBytes: UInt64 = 0
    var fixtureDownloadsSeeded = false
    var downloadObserver: NSObjectProtocol?
    var fixtureQueueSeeded = false
    var fixtureContextsSeeded = false
    var loadGeneration = 0
    var player = AVQueuePlayer()
    var core: NativeCore?
    var audioObservers: [NSObjectProtocol] = []
    var resumeAfterInterruption = false
    var observer: NSObjectProtocol?
    var managedItems: Set<ObjectIdentifier> = []
    var itemObservers: [ObjectIdentifier: NSKeyValueObservation] = [:]
    /// The saved position to restore once the rebuilt current item is ready.
    var pendingResume: (item: ObjectIdentifier, position: Double)?
    var downloads: BackgroundDownloads?
    var fixture: Bool { nativeFixtureMode() }
    static let searchPages: Set<String> = ["Omnibar", "Search", "New music"]
    static let collectionKinds: Set<String> = ["albums", "artists", "playlists", "genres", "newAlbums"]
    init() {
        core = NativeEnvironment.shared.core
        downloads = NativeEnvironment.shared.downloads
        if let core {
            let snapshot = decodeJSON((try? core.queueCommand(command: "{}")) ?? "{}")
            autoplay = snapshot["autoplay"] as? Bool ?? true
            applyModes(snapshot)
            // Show the restored (paused) track like other music apps; playback still waits for Play.
            if let song = snapshot["current"] as? [String: Any] { currentSong = song; current = song["title"] as? String ?? current }
        }
        downloadObserver = NotificationCenter.default.addObserver(forName: .nativeDownloadsChanged, object: nil, queue: .main) { [weak self] _ in
            Task { @MainActor in self?.refreshDownloadCatalog(); self?.refreshAttention() }
        }
        observePlaybackClock()
        // The session is activated when playback starts, so opening the app never stops other audio.
        do { try AVAudioSession.sharedInstance().setCategory(.playback, mode: .default) } catch { self.error = "Audio session unavailable" }
        configureRemoteCommands()
        observeAudioSession()
        observer = NotificationCenter.default.addObserver(forName: .AVPlayerItemDidPlayToEndTime, object: nil, queue: .main) { [weak self] notification in
            Task { @MainActor in self?.didFinish(notification.object as? AVPlayerItem) }
        }
        audioObservers.append(NotificationCenter.default.addObserver(forName: .AVPlayerItemFailedToPlayToEndTime, object: nil, queue: .main) { [weak self] notification in
            let error = notification.userInfo?[AVPlayerItemFailedToPlayToEndTimeErrorKey] as? Error
            Task { @MainActor in self?.itemFailed(notification.object as? AVPlayerItem, error: error) }
        })
        refreshAttention()
    }
    private func observePlaybackClock() {
        timeObserver = player.addPeriodicTimeObserver(forInterval: CMTime(seconds: 5, preferredTimescale: 600), queue: .main) { [weak self] _ in
            Task { @MainActor in self?.reportProgress(); self?.savePositionWhilePlaying(); self?.nowPlaying(); self?.requestRadio() }
        }
    }
    private func configureRemoteCommands() {
        let commands = MPRemoteCommandCenter.shared()
        commands.playCommand.addTarget { [weak self] _ in Task { @MainActor in self?.resume() }; return .success }
        commands.pauseCommand.addTarget { [weak self] _ in Task { @MainActor in self?.pause() }; return .success }
        commands.togglePlayPauseCommand.addTarget { [weak self] _ in Task { @MainActor in self?.togglePlayback() }; return .success }
        commands.stopCommand.addTarget { [weak self] _ in Task { @MainActor in self?.pause(); self?.seek(to: 0) }; return .success }
        commands.nextTrackCommand.addTarget { [weak self] _ in Task { @MainActor in self?.advance() }; return .success }
        commands.previousTrackCommand.addTarget { [weak self] _ in Task { @MainActor in self?.previous() }; return .success }
        commands.changePlaybackPositionCommand.addTarget { [weak self] event in
            guard let event = event as? MPChangePlaybackPositionCommandEvent else { return .commandFailed }
            let position = event.positionTime
            Task { @MainActor in self?.seek(to: position) }
            return .success
        }
    }
    private func observeAudioSession() {
        let center = NotificationCenter.default
        audioObservers.append(center.addObserver(forName: AVAudioSession.interruptionNotification, object: nil, queue: .main) { [weak self] note in
            guard let raw = (note.userInfo?[AVAudioSessionInterruptionTypeKey] as? NSNumber)?.uintValue,
                  let type = AVAudioSession.InterruptionType(rawValue: raw) else { return }
            let options = (note.userInfo?[AVAudioSessionInterruptionOptionKey] as? NSNumber)?.uintValue ?? 0
            let shouldResume = AVAudioSession.InterruptionOptions(rawValue: options).contains(.shouldResume)
            Task { @MainActor in self?.interruption(began: type == .began, shouldResume: shouldResume) }
        })
        audioObservers.append(center.addObserver(forName: AVAudioSession.routeChangeNotification, object: nil, queue: .main) { [weak self] note in
            // Unplugging headphones or losing a Bluetooth/CarPlay output pauses, as in every iOS music app.
            let reason = (note.userInfo?[AVAudioSessionRouteChangeReasonKey] as? NSNumber)?.uintValue
            guard reason == AVAudioSession.RouteChangeReason.oldDeviceUnavailable.rawValue else { return }
            Task { @MainActor in self?.pause() }
        })
        audioObservers.append(center.addObserver(forName: AVAudioSession.mediaServicesWereResetNotification, object: nil, queue: .main) { [weak self] _ in
            Task { @MainActor in self?.mediaServicesReset() }
        })
    }
    /// Calls, alarms and Siri pause playback; it resumes only when we were playing and iOS allows it.
    func interruption(began: Bool, shouldResume: Bool) {
        if began {
            // A suspended app can receive a stale "began" after waking; ignore it when idle.
            guard playing || wantsPlayback else { return }
            resumeAfterInterruption = wantsPlayback
            reportProgress()
            savePosition()
            player.pause(); playing = false; nowPlaying()
        } else {
            let restart = resumeAfterInterruption && shouldResume
            resumeAfterInterruption = false
            if restart { resume() } else if !playing { wantsPlayback = false; nowPlaying() }
        }
    }
    /// The rebuilt item resumes from the saved position, so recovery never restarts the song.
    func mediaServicesReset() {
        savePosition()
        if let timeObserver { player.removeTimeObserver(timeObserver) }
        timeObserver = nil
        player.pause()
        forgetItems()
        player.removeAllItems()
        // Objects created before a media-server reset are invalid; recreate the native player.
        player = AVQueuePlayer()
        observePlaybackClock()
        playing = false
        wantsPlayback = false
        resumeAfterInterruption = false
        try? AVAudioSession.sharedInstance().setCategory(.playback, mode: .default)
        rebuild(start: false)
    }
    func command(_ value: [String: Any]) -> [String: Any] {
        guard let core else { error = "Cannot open local catalog"; return [:] }
        do {
            let result = decodeJSON(try core.queueCommand(command: encodeJSON(value)))
            applyModes(result)
            if let op = value["op"] as? String, ["shuffle", "repeat", "playNext", "playLast", "append", "remove", "move", "removeSecondary", "moveSecondary", "moveUpcoming"].contains(op) {
                refreshUpcoming()
            }
            return result
        }
        catch { self.error = "Queue could not be updated"; return [:] }
    }
    func applyModes(_ snapshot: [String: Any]) {
        volume = (snapshot["volume"] as? NSNumber)?.doubleValue ?? 1
        playbackEpoch = (snapshot["playbackEpoch"] as? NSNumber)?.uint64Value ?? 0
        let radio = snapshot["radioError"] as? String ?? ""
        if radio != radioError { radioError = radio }
        guard let state = snapshot["state"] as? [String: Any] else { return }
        let shuffle = state["shuffle"] as? Bool ?? false
        let repeatValue = state["repeat"] as? String ?? "off"
        if shuffle != shuffleOn { shuffleOn = shuffle }
        if repeatValue != repeatMode { repeatMode = repeatValue }
    }
    var elapsed: Double { let time = player.currentTime().seconds; return time.isFinite ? max(0, time) : 0 }
    var duration: Double {
        let time = player.currentItem?.duration.seconds ?? .nan
        return time.isFinite && time > 0 ? time : (currentSong["duration"] as? Double ?? 0)
    }
    var progressFraction: Double { duration > 0 ? min(1, elapsed / duration) : 0 }
    func seek(to seconds: Double) {
        player.seek(to: CMTime(seconds: max(0, seconds), preferredTimescale: 600))
        savePosition(max(0, seconds))
        nowPlaying()
    }
    /// The core ignores reports from an older epoch or song, so late clocks never overwrite a newer track.
    func savePosition(_ position: Double? = nil) {
        guard let core, let id = currentSong["id"] as? String, player.currentItem != nil else { return }
        try? core.savePlaybackPosition(epoch: playbackEpoch, songId: id, position: position ?? elapsed)
    }
    func savePositionWhilePlaying() {
        guard playing, player.timeControlStatus == .playing, player.rate > 0 else { return }
        savePosition()
    }
    /// Cover art through the shared core cache; nil means "keep the placeholder".
    func artwork(_ item: [String: Any], size: Int) async -> UIImage? {
        let source = [item["artworkUrl"], item["coverArt"], item["albumId"]].compactMap { $0 as? String }.filter { !$0.isEmpty }
        guard let core, !source.isEmpty else { return nil }
        let key = "\(size)|\(source.joined(separator: "|"))" as NSString
        if let cached = artCache.object(forKey: key) { return cached }
        let json = encodeJSON(item)
        let data = await Task.detached(priority: .utility) { try? core.artwork(itemJson: json, size: UInt32(size)) }.value
        guard let data, let image = UIImage(data: data) else { return nil }
        artCache.setObject(image, forKey: key)
        return image
    }
    func setCurrent(_ song: [String: Any]?) {
        currentSong = song ?? [:]
        current = song?["title"] as? String ?? "Nothing playing"
        nowPlayingArt = nil
        guard let song else { return }
        Task {
            let image = await self.artwork(song, size: 600)
            guard self.currentSong["id"] as? String == song["id"] as? String else { return }
            self.nowPlayingArt = image
            self.nowPlaying()
        }
    }
    // Navigation state belongs to each native tab; the playback engine remains shared.
    func captureCatalog() -> [String: Any] {
        // A chip-filtered search keeps its full results so the chip can be changed after Back.
        let filtered = Self.searchPages.contains(page) && searchFilter != "all"
        var extra: [String: Any] = ["sections": filtered ? searchSourceSections : sections, "contexts": recentContexts,
                                    "suggestions": suggestions, "correction": correction, "readyBytes": storageBytes]
        if !resolvedKind.isEmpty { extra["kind"] = resolvedKind; extra["title"] = resolvedTitle }
        return ["page": page, "title": headerTitle, "kind": kind, "rows": filtered ? searchSourceRows : rows, "query": searchQuery,
                "context": collectionContext, "offline": offline, "error": error, "notice": notice, "trail": trail,
                "searchFilter": searchFilter, "jobFilter": jobFilter, "extra": extra]
    }
    func restoreCatalog(_ snapshot: [String: Any]) {
        loadGeneration += 1 // Ignore responses from the tab that is no longer visible.
        loading = false
        page = snapshot["page"] as? String ?? "Home"
        error = snapshot["error"] as? String ?? ""
        notice = snapshot["notice"] as? String ?? ""
        searchQuery = snapshot["query"] as? String ?? ""
        trail = snapshot["trail"] as? [[String: Any]] ?? []
        searchFilter = snapshot["searchFilter"] as? String ?? "all"
        jobFilter = snapshot["jobFilter"] as? String ?? "all"
        applyCatalog(snapshot)
    }
    func pushTrail() {
        var snapshot = captureCatalog()
        snapshot["trail"] = nil
        trail.append(snapshot)
    }
    func back() {
        guard let previous = trail.last else { return }
        let remaining = Array(trail.dropLast())
        restoreCatalog(previous)
        trail = remaining
    }
    /// `refresh` reloads the visible page in place: rows, trail and notices stay until the response arrives.
    func load(_ page: String, query: String = "", refresh: Bool = false, keepTrail: Bool = false) async {
        searchQuery = query
        if !refresh {
            self.page = page; headerTitle = page; collectionContext = [:]; sections = []; suggestions = []; correction = ""; rows = []; notice = ""
            resolvedKind = ""; resolvedTitle = ""; jobFilter = "all"
            if !keepTrail { trail = [] }
        }
        error = ""
        loadGeneration += 1
        let generation = loadGeneration
        // An empty library search shows recent listening from the core; New music needs a query.
        let emptySearch = Self.searchPages.contains(page) && query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        if emptySearch && page == "New music" {
            kind = "songs"; rows = []; sections = []; suggestions = []; searchSourceRows = []; searchSourceSections = []; loading = false
            return
        }
        if fixture && ["Home", "Liked Songs"].contains(page) { loadFixture(page); return }
        if fixture, let core { seedFixture(page, core: core) }
        guard let core else { error = "Cannot open local catalog"; return }
        if !refresh { loading = true }
        do {
            let text = try await Task.detached { try core.browse(page: page, query: query) }.value
            guard generation == loadGeneration else { return }
            loading = false
            applyCatalog(decodeJSON(text))
            // Opening Jobs re-read the server's failures in the core.
            if kind == "jobs" { refreshAttention() }
        } catch {
            guard generation == loadGeneration else { return }
            loading = false
            // Recents are a convenience; an unreachable server must not greet an empty search with an error.
            if !emptySearch { self.error = "Could not reach server. Check Tailscale or open Downloads." }
        }
    }
    func fixtureSongs() -> [[String: Any]] {
        // Mirrors the loopback fixture server, whose generated covers exercise real artwork loading.
        (1...8).map { (index: Int) -> [String: Any] in
            let album = (index + 1) / 2
            return ["id": "fixture-\(index)", "title": "Fixture song \(index)", "artist": "ZayMusic", "album": "Fixture album \(album)", "albumId": "fixture-album-\(album)", "coverArt": "al-fixture-\(album)", "duration": 120 + index * 17]
        }
    }
    private func loadFixture(_ page: String) {
        kind = "songs"; loading = false
        rows = fixtureSongs()
        guard page == "Home" else { return }
        sections = [["title": "Your top songs", "start": 0, "count": 5], ["title": "Rediscover", "start": 5, "count": 3]]
        guard let core else { return }
        if !fixtureContextsSeeded {
            try? core.recordContext(contextJson: encodeJSON(["kind": "albums", "id": "fixture-album-1", "name": "Fixture album 1", "artist": "ZayMusic", "coverArt": "al-fixture-1"]))
            fixtureContextsSeeded = true
        }
        recentContexts = decodeRows((try? core.recentContexts()) ?? "[]")
    }
    private func seedFixture(_ page: String, core: NativeCore) {
        if page == "Queue" && !fixtureQueueSeeded {
            let songs = fixtureSongs()
            _ = try? core.queueCommand(command: encodeJSON(["op": "set", "songs": songs]))
            _ = try? core.queueCommand(command: encodeJSON(["op": "playNext", "songs": [songs[2]]]))
            fixtureQueueSeeded = true
        }
        if page == "Downloads" && !fixtureDownloadsSeeded {
            let plan = decodeJSON((try? core.queueDownload(songJson: encodeJSON(["id": "offline-fixture", "title": "Offline fixture", "artist": "ZayMusic"]))) ?? "{}")
            if let ticket = plan["ticket"] as? UInt64 { try? core.failDownload(id: "offline-fixture", ticket: ticket, reason: "transport") }
            fixtureDownloadsSeeded = true
        }
    }
    func applyCatalog(_ value: [String: Any]) {
        let extra = value["extra"] as? [String: Any] ?? [:]
        collectionContext = value["context"] as? [String: Any] ?? [:]
        headerTitle = value["title"] as? String ?? page
        rows = value["rows"] as? [[String: Any]] ?? []
        kind = value["kind"] as? String ?? "songs"
        offline = value["offline"] as? Bool ?? false
        recentContexts = extra["contexts"] as? [[String: Any]] ?? []
        suggestions = (extra["suggestions"] as? [Any] ?? []).compactMap { $0 as? String }.filter { !$0.isEmpty }
        correction = searchCorrection(extra["correction"])
        sections = extra["sections"] as? [[String: Any]] ?? []
        // Resolved YouTube playlist and album links list their tracks for bulk selection.
        let resolved = extra["kind"] as? String ?? ""
        resolvedKind = ["playlist", "album"].contains(resolved) ? resolved : ""
        resolvedTitle = resolvedKind.isEmpty ? "" : extra["title"] as? String ?? ""
        if Self.searchPages.contains(value["page"] as? String ?? page) {
            searchSourceRows = rows; searchSourceSections = sections
            applySearchFilter()
        }
        if let bytes = extra["readyBytes"] as? UInt64 { storageBytes = bytes }
    }
    func applySearchFilter() {
        let shown = filterSearch(searchSourceRows, sections: searchSourceSections, filter: searchFilter)
        rows = shown.rows; sections = shown.sections
    }
    func setSearchFilter(_ filter: String) {
        guard filter != searchFilter else { return }
        // Tests and older snapshots may have set rows directly; treat them as the full results.
        if searchFilter == "all" { searchSourceRows = rows; searchSourceSections = sections }
        searchFilter = filter
        applySearchFilter()
    }
    /// The catalog kind of one row: mixed search rows carry their own, recent collections name theirs.
    func rowKind(_ row: [String: Any]) -> String {
        if let browse = row["browseKind"] as? String { return browse }
        if kind == "contexts" { return row["kind"] as? String ?? kind }
        return kind
    }
    /// Songs on the visible page; album, artist and new-album rows never enter the queue.
    var playable: [[String: Any]] { kind == "songs" ? playableRows(rows) : [] }
    var downloadable: [[String: Any]] { kind == "songs" ? downloadableRows(rows) : [] }
    var sectionRanges: [Range<Int>] {
        sections.compactMap { section in
            guard let start = section["start"] as? Int, let count = section["count"] as? Int,
                  start >= 0, count >= 0, start + count <= rows.count else { return nil }
            return start..<(start + count)
        }
    }
    func select(_ row: [String: Any], at position: Int? = nil) {
        let source = rowKind(row)
        if source == "liked" { pushTrail(); Task { await load("Liked Songs", keepTrail: true) }; return }
        if Self.collectionKinds.contains(source) { openCollection(source, row: row); return }
        guard source == "songs" else { return }
        if let lane = row["queueLane"] as? String {
            _ = command(["op": lane == "priority" ? "jumpSecondary" : "jump", "index": row["queueIndex"] ?? 0])
            rebuild(); return
        }
        let selected = position ?? rows.firstIndex { ($0["id"] as? String) == (row["id"] as? String) } ?? 0
        // Home shelves and search groups play as their own context, like Spotify.
        guard let context = playbackContext(rows, sections: sectionRanges, selected: selected), let core else { return }
        // The core keeps the listener's shuffle except for albums, and records the collection.
        do {
            applyModes(decodeJSON(try core.playRows(songsJson: encodeJSON(context.songs), index: UInt32(context.index), contextJson: encodeJSON(currentPlayContext))))
            rebuild()
        } catch { playbackError = "Could not start playback." }
    }
    var currentPlayContext: [String: Any] { playContextValue(page: page, collection: collectionContext) }
    /// The playing song as a song row, whatever kind of page the tab underneath shows.
    var currentSongRow: [String: Any] { var song = currentSong; if !song.isEmpty { song["browseKind"] = "songs" }; return song }
    /// Collection Play: playlists and Liked Songs start shuffled at a random song; albums start in order.
    func playCollection() {
        let songs = playable
        guard let core, !songs.isEmpty else { return }
        let songsJson = encodeJSON(songs)
        let contextJson = encodeJSON(currentPlayContext)
        wantsPlayback = true
        Task {
            do {
                let snapshot = decodeJSON(try await Task.detached { try core.playCollection(songsJson: songsJson, contextJson: contextJson) }.value)
                self.applyModes(snapshot)
                self.rebuild(start: true)
            } catch { self.playbackError = "Could not start playback." }
        }
    }
    /// Plays a Home shortcut without opening it; the core loads the collection's songs.
    func playShortcut(_ item: [String: Any]) {
        guard let core else { return }
        let contextJson = encodeJSON(item)
        wantsPlayback = true
        playbackError = ""
        Task {
            do {
                let snapshot = decodeJSON(try await Task.detached { try core.playContext(contextJson: contextJson) }.value)
                self.applyModes(snapshot)
                self.rebuild(start: true)
            } catch { self.wantsPlayback = false; self.playbackError = "Could not play \(rowTitle(item)). Check the connection and retry." }
        }
    }
    /// `keepTrail: false` starts a fresh trail for collections pushed by the system navigation stack.
    func openCollection(_ source: String, row: [String: Any], keepTrail: Bool = true) {
        guard let core else { return }
        let id = row["id"] as? String ?? row["value"] as? String ?? ""
        guard !id.isEmpty else { return }
        if keepTrail { pushTrail() } else { trail = [] }
        loadGeneration += 1
        let generation = loadGeneration
        // Show the header immediately; tracks follow from the shared core.
        var context: [String: Any] = ["kind": source, "id": id, "name": rowTitle(row)]
        for key in ["coverArt", "artworkUrl", "artist", "starred"] where present(row[key]) { context[key] = row[key] }
        page = "Collection"; headerTitle = rowTitle(row); collectionContext = context
        kind = source == "artists" ? "albums" : "songs"
        rows = []; sections = []; suggestions = []; correction = ""; resolvedKind = ""; resolvedTitle = ""
        recentContexts = []; offline = false; error = ""; notice = ""
        loading = true
        Task {
            do {
                let text = try await Task.detached { try core.openCollection(kind: source, id: id) }.value
                guard generation == self.loadGeneration else { return }
                self.loading = false
                self.applyCatalog(decodeJSON(text))
            } catch {
                guard generation == self.loadGeneration else { return }
                self.loading = false
                self.error = "Could not open collection. Check Tailscale or open Downloads."
            }
        }
    }
    /// Imports → failed tracks: Jobs filtered to what needs attention; Back returns to Imports.
    func showFailedJobs() {
        pushTrail()
        Task { await load("Jobs", keepTrail: true); jobFilter = "attention" }
    }
    func refreshDownloadCatalog() {
        if let core, let text = try? core.storageStats() { storageBytes = decodeJSON(text)["readyBytes"] as? UInt64 ?? 0 }
        if page == "Downloads" { Task { guard self.page == "Downloads" else { return }; await load("Downloads", refresh: true) } }
    }
    func downloadShown() {
        let songs = downloadable
        guard !songs.isEmpty, let manager = downloads else { return }
        Task.detached { manager.enqueueMany(songs) }
        notice = "Downloading \(songs.count) song\(songs.count == 1 ? "" : "s")"
    }
    func downloadCollection(_ row: [String: Any]) {
        let source = rowKind(row)
        guard let core, ["albums", "playlists"].contains(source), let id = row["id"] as? String, !id.isEmpty else { return }
        Task {
            do {
                let value = decodeJSON(try await Task.detached { try core.openCollection(kind: source, id: id) }.value)
                let songs = value["kind"] as? String == "songs" ? downloadableRows(value["rows"] as? [[String: Any]] ?? []) : []
                guard !songs.isEmpty, let manager = self.downloads else { self.error = "Nothing to download"; return }
                Task.detached { manager.enqueueMany(songs) }
                self.notice = "Downloading \(songs.count) song\(songs.count == 1 ? "" : "s")"
            } catch { self.error = "Could not download collection" }
        }
    }
    func requestRadio() {
        guard !fixture, wantsPlayback, radioTask == nil, let core else { return }
        radioTask = Task {
            let extended = await Task.detached { (try? core.ensureRadio()) ?? false }.value
            self.radioTask = nil
            // Surface the core's radio error (or clear it) without changing the queue.
            self.applyModes(decodeJSON((try? core.queueCommand(command: "{}")) ?? "{}"))
            guard self.wantsPlayback else { return }
            if self.resumeAfterRadio && extended {
                self.resumeAfterRadio = false
                if self.command(["op": "next"])["continue"] as? Bool == true { self.rebuild() }
            } else if extended { self.topUpUpcoming() }
        }
    }
    func setAutoplay(_ enabled: Bool) {
        autoplay = enabled
        try? core?.setAutoplay(enabled: enabled)
        if enabled { requestRadio() }
    }
    func topUpUpcoming() {
        guard let core, player.currentItem != nil else { return }
        let snapshot = decodeJSON((try? core.queueCommand(command: "{}")) ?? "{}")
        let existing = max(0, player.items().count - 1)
        for next in (snapshot["upcoming"] as? [[String: Any]] ?? []).prefix(2).dropFirst(existing) {
            if let text = try? core.mediaUrl(songJson: encodeJSON(next), download: false), let url = URL(string: text) {
                player.insert(makeItem(url), after: player.items().last)
            }
        }
        Task.detached { try? core.warmUpcoming() }
    }
    func shuffle(_ density: String) {
        let songs = playable
        guard let core, !songs.isEmpty else { return }
        let source = encodeJSON(songs)
        recordPlayContext()
        wantsPlayback = true
        Task {
            do {
                let result = decodeJSON(try await Task.detached { try core.startShuffle(songsJson: source, density: density) }.value)
                let snapshot = decodeJSON(try core.queueCommand(command: "{}"))
                if result["applied"] as? Bool == true, result["generation"] as? UInt64 == snapshot["generation"] as? UInt64 {
                    self.error = result["warning"] as? String ?? ""
                    self.rebuild(start: self.wantsPlayback)
                }
            } catch { self.error = "Could not prepare shuffle" }
        }
    }
    /// Edits one queue lane. Only a visible Queue page reloads; other tabs keep their catalog.
    func queueAction(_ operation: String, row: [String: Any], delta: Int = 0) {
        guard let lane = row["queueLane"] as? String, let index = row["queueIndex"] as? Int else { return }
        let value: [String: Any]
        switch (operation, lane == "priority") {
        case ("remove", true): value = ["op": "removeSecondary", "index": index]
        case ("remove", false): value = ["op": "remove", "index": index]
        case ("move", true): value = ["op": "moveSecondary", "from": index, "to": max(0, index + delta)]
        case ("move", false): value = ["op": "moveUpcoming", "from": index, "direction": delta]
        default: return
        }
        let before = (command([:])["current"] as? [String: Any])?["id"] as? String
        let after = (command(value)["current"] as? [String: Any])?["id"] as? String
        if before != after { rebuild(start: wantsPlayback) }
        if page == "Queue" { Task { await load("Queue", refresh: true) } }
    }
    func refreshUpcoming() {
        guard let core, let currentItem = player.currentItem else { return }
        for item in player.items() where item !== currentItem { player.remove(item) }
        forgetItems(except: currentItem)
        let snapshot = decodeJSON((try? core.queueCommand(command: "{}")) ?? "{}")
        for next in (snapshot["upcoming"] as? [[String: Any]] ?? []).prefix(2) {
            if let text = try? core.mediaUrl(songJson: encodeJSON(next), download: false), let url = URL(string: text) {
                player.insert(makeItem(url), after: player.items().last)
            }
        }
        Task.detached { try? core.warmUpcoming() }
    }
    /// Every queued AVPlayerItem is tracked so stale end and failure events are ignored.
    func makeItem(_ url: URL) -> AVPlayerItem {
        let item = AVPlayerItem(url: url)
        let key = ObjectIdentifier(item)
        managedItems.insert(key)
        itemObservers[key] = item.observe(\.status, options: [.new]) { [weak self] item, _ in
            let status = item.status
            let error = item.error
            Task { @MainActor in
                if status == .failed { self?.itemFailed(item, error: error) } else if status == .readyToPlay { self?.itemReady(item) }
            }
        }
        return item
    }
    func forgetItems(except kept: AVPlayerItem? = nil) {
        let key = kept.map { ObjectIdentifier($0) }
        for (id, observation) in itemObservers where id != key { observation.invalidate(); itemObservers[id] = nil }
        managedItems = key.map { Set([$0]) } ?? []
        if pendingResume?.item != key { pendingResume = nil }
    }
    func itemReady(_ item: AVPlayerItem) {
        guard let pending = pendingResume, pending.item == ObjectIdentifier(item), player.currentItem === item else { return }
        pendingResume = nil
        if elapsed < pending.position - 1 { player.seek(to: CMTime(seconds: pending.position, preferredTimescale: 600)) }
    }
    /// A failed item never leaves the UI claiming to play. Broken preloads are dropped and
    /// rebuilt when the queue reaches them.
    func itemFailed(_ item: AVPlayerItem?, error: Error?) {
        guard let item, managedItems.contains(ObjectIdentifier(item)) else { return }
        guard player.currentItem === item else {
            player.remove(item)
            managedItems.remove(ObjectIdentifier(item)); itemObservers.removeValue(forKey: ObjectIdentifier(item))?.invalidate()
            return
        }
        let local = (item.asset as? AVURLAsset)?.url.isFileURL ?? false
        let preview = (currentSong["id"] as? String)?.hasPrefix("yt:") == true
        player.pause()
        playing = false; wantsPlayback = false; resumeAfterRadio = false
        playbackError = playbackFailureMessage(error, preview: preview, local: local)
        nowPlaying()
    }
    func rebuild(start: Bool = true) {
        wantsPlayback = start
        resumeAfterRadio = false
        playbackError = ""
        player.pause(); forgetItems(); player.removeAllItems()
        guard let core else { return }
        let snapshot = command([:])
        guard let song = snapshot["current"] as? [String: Any] else { playing = false; wantsPlayback = false; setCurrent(nil); nowPlaying(); return }
        do {
            let url = try core.mediaUrl(songJson: encodeJSON(song), download: false)
            guard let url = URL(string: url) else { throw CocoaError(.fileReadInvalidFileName) }
            let item = makeItem(url)
            player.insert(item, after: nil)
            setCurrent(song)
            // Launch, recovery and rebuilt queues continue from the core's saved position.
            let resumeAt = (snapshot["resumePosition"] as? NSNumber)?.doubleValue ?? 0
            if resumeAt > 0 {
                pendingResume = (ObjectIdentifier(item), resumeAt)
                player.seek(to: CMTime(seconds: resumeAt, preferredTimescale: 600))
            }
            // Prime the actual next queue item so AVFoundation can transition while suspended.
            let upcoming = snapshot["upcoming"] as? [[String: Any]] ?? []
            for next in upcoming.prefix(2) {
                if let nextURL = URL(string: try core.mediaUrl(songJson: encodeJSON(next), download: false)) { player.insert(makeItem(nextURL), after: player.items().last) }
            }
            player.volume = (snapshot["gain"] as? NSNumber)?.floatValue ?? 1
            Task.detached { try? core.warmUpcoming() }
            if start { resume() } else { playing = false; nowPlaying() }
        } catch {
            playing = false; wantsPlayback = false
            if player.currentItem == nil { setCurrent(song) }
            playbackError = (song["id"] as? String)?.hasPrefix("yt:") == true ? "This preview can't be played right now." : "This song can't be played right now. Check the connection and retry."
            nowPlaying()
        }
    }
    func didFinish(_ item: AVPlayerItem?) {
        guard let item, managedItems.remove(ObjectIdentifier(item)) != nil else { return }
        itemObservers.removeValue(forKey: ObjectIdentifier(item))?.invalidate()
        let completedDuration = item.duration.seconds
        let end = completedDuration.isFinite && completedDuration > 0 ? completedDuration : (currentSong["duration"] as? Double ?? 0)
        reportProgress(position: end, duration: end, finished: true)
        let advanced = command(["op": "next"])
        if advanced["continue"] as? Bool != true {
            playing = false; resumeAfterRadio = wantsPlayback; nowPlaying(); requestRadio(); return
        }
        // A dropped preload leaves AVFoundation without a next item; rebuild from the core.
        if player.currentItem == nil { rebuild(start: wantsPlayback); return }
        let snapshot = command([:])
        if let song = snapshot["current"] as? [String: Any] { setCurrent(song) }
        player.volume = (snapshot["gain"] as? NSNumber)?.floatValue ?? 1
        if let core, let upcoming = snapshot["upcoming"] as? [[String: Any]], upcoming.count > 1,
           let text = try? core.mediaUrl(songJson: encodeJSON(upcoming[1]), download: false), let url = URL(string: text) {
            player.insert(makeItem(url), after: player.items().last)
        }
        nowPlaying()
        requestRadio()
    }
    func resume() {
        if player.currentItem == nil || player.currentItem?.status == .failed { rebuild(); return }
        try? AVAudioSession.sharedInstance().setActive(true)
        playbackError = ""
        player.play(); playing = true; wantsPlayback = true; nowPlaying(); requestRadio()
    }
    func pause() { savePosition(); player.pause(); playing = false; wantsPlayback = false; nowPlaying() }
    func togglePlayback() { playing ? pause() : resume() }
    func advance() { let start = playing; let v = command(["op": "next"]); if v["continue"] as? Bool == true { rebuild(start: start) } else { playing = false; resumeAfterRadio = wantsPlayback; requestRadio() } }
    /// Past three seconds the core restarts the song instead of going back (it says `restart`).
    func previous() {
        let start = playing
        if command(["op": "previous", "position": elapsed])["restart"] as? Bool == true { seek(to: 0) } else { rebuild(start: start) }
    }
    /// Re-reads the playing song's metadata after likes or tag edits without touching audio.
    func refreshCurrentMetadata(_ id: String, patch: [String: Any?] = [:]) {
        guard currentSong["id"] as? String == id else { return }
        var song = (command([:])["current"] as? [String: Any]).flatMap { $0["id"] as? String == id ? $0 : nil } ?? currentSong
        for (key, value) in patch { song[key] = value ?? NSNull() }
        currentSong = song
        current = song["title"] as? String ?? current
        nowPlaying()
    }
    func refreshAttention() {
        guard let core else { attentionCount = 0; return }
        let total = (decodeJSON((try? core.attentionSummary(refresh: false)) ?? "{}")["total"] as? NSNumber)?.intValue ?? 0
        if total != attentionCount { attentionCount = total }
    }
    /// Re-reads failed server jobs, so the badge never waits for Jobs to open.
    func checkServerAttention() async {
        guard let core else { return }
        _ = try? await Task.detached { try core.attentionSummary(refresh: true) }.value
        refreshAttention()
    }
    func reportProgress(position: Double? = nil, duration: Double? = nil, finished: Bool = false) {
        guard let core, let id = currentSong["id"] as? String else { return }
        try? core.playbackProgress(epoch: playbackEpoch, songId: id, position: position ?? elapsed, duration: duration ?? self.duration, playing: finished || (playing && player.timeControlStatus == .playing && player.rate > 0))
    }
    func nowPlaying() {
        var info: [String: Any] = [MPMediaItemPropertyTitle: current, MPNowPlayingInfoPropertyElapsedPlaybackTime: elapsed, MPNowPlayingInfoPropertyPlaybackRate: playing ? 1.0 : 0.0]
        if let artist = currentSong["artist"] as? String, !artist.isEmpty { info[MPMediaItemPropertyArtist] = artist }
        if let album = currentSong["album"] as? String, !album.isEmpty { info[MPMediaItemPropertyAlbumTitle] = album }
        if duration > 0 { info[MPMediaItemPropertyPlaybackDuration] = duration }
        if let art = nowPlayingArt { info[MPMediaItemPropertyArtwork] = lockScreenArtwork(art) }
        MPNowPlayingInfoCenter.default().nowPlayingInfo = info
    }
    func action(_ op: String, _ row: [String: Any]) {
        let id = row["id"] as? String ?? ""
        switch op {
        case "download":
            if rowKind(row) == "songs" { if isLibrarySong(row) { downloads?.enqueue(row) } } else { downloadCollection(row) }
        case "cancelDownload": if !id.isEmpty { downloads?.cancel(id) }
        case "removeDownload": if !id.isEmpty { downloads?.remove(id) }
        case "acquire":
            guard let core, rowKind(row) == "songs", id.hasPrefix("yt:") else { return }
            let json = encodeJSON(row)
            Task {
                do {
                    _ = try await Task.detached { try core.acquireSong(songJson: json) }.value
                    self.notice = "Adding to your library. Track progress in Jobs."
                } catch { self.error = "Could not add song to library. Retry from Jobs." }
            }
        case "playNext", "playLast", "append":
            guard rowKind(row) == "songs" else { return }
            _ = command(["op": op, "songs": [row]])
            notice = op == "append" ? "Added to queue" : op == "playNext" ? "Playing next" : "Added to Next in queue"
            if page == "Queue" { Task { await load("Queue", refresh: true) } }
        default: break
        }
    }
}

@main struct ZayMusicApp: App {
    @UIApplicationDelegateAdaptor(NativeAppDelegate.self) var appDelegate
    @StateObject var music = Music()
    // White chrome like familiar music apps; green is applied only to play/now-playing/on states.
    var body: some Scene { WindowGroup { ContentView().environmentObject(music).preferredColorScheme(.dark).tint(.white) } }
}
/// A tab's native navigation destination survives tab switches. Reloading on every
/// appearance would overwrite its restored collection with the original list page.
private struct LoadCatalogOnFirstAppearance: ViewModifier {
    @EnvironmentObject private var music: Music
    let page: String
    let query: String
    @State private var didLoad = false

    init(page: String, query: String = "") {
        self.page = page
        self.query = query
    }
    func body(content: Content) -> some View {
        content.task {
            guard !didLoad else { return }
            await music.load(page, query: query)
            // A cancelled first visit must be allowed to load on the next visit.
            if !Task.isCancelled { didLoad = true }
        }
    }
}
/// Library items open as pushed screens with a fresh trail, so the system Back pops them.
private struct OpenCollectionOnFirstAppearance: ViewModifier {
    @EnvironmentObject private var music: Music
    let source: String
    let row: [String: Any]
    @State private var didOpen = false
    func body(content: Content) -> some View {
        content.onAppear {
            guard !didOpen else { return }
            didOpen = true
            music.openCollection(source, row: row, keepTrail: false)
        }
    }
}
/// Collections open in place; while one is shown, Back returns to the page it came from.
private struct CatalogBackButton: ViewModifier {
    @EnvironmentObject private var music: Music
    func body(content: Content) -> some View {
        content
            .navigationBarBackButtonHidden(!music.trail.isEmpty)
            .toolbar {
                if !music.trail.isEmpty {
                    ToolbarItem(placement: .topBarLeading) {
                        Button { music.back() } label: {
                            Image(systemName: "chevron.backward").fontWeight(.semibold)
                        }
                        .accessibilityLabel("Back")
                        .accessibilityIdentifier("catalog-back")
                    }
                }
            }
    }
}

struct ContentView: View {
    @EnvironmentObject var music: Music
    @State private var pendingSongDeletion: [String: Any]?
    @State var query = ""
    @FocusState private var searchFocused: Bool
    @State private var selectedTab = "Home"
    @State private var tabCatalogs: [String: [String: Any]] = [:]
    @State var sheet = false
    @State var clearDownloads = false
    @State var collectionActions = false
    @State var pendingCollectionAction: (() -> Void)?
    @State var playlistSelection: PlaylistSelection?
    @State var playlistEditor = false
    @State var playlistName = ""
    @State var editingPlaylistId: String?
    @State var deletingPlaylist = false
    @State var deletingPlaylistId = ""
    @State var tagEditing: TagSelection?
    /// Bulk selection over a resolved YouTube playlist or album link.
    @State var selecting = false
    @State var selectedIds: Set<String> = []
    @State private var searchTask: Task<Void, Never>?
    /// "Go to album/artist" from the player opens in a tab whose root shows the catalog.
    @State private var pendingNavigation: (source: String, row: [String: Any])?
    @State var libraryFilter = "all"
    @State var librarySort = "recent"
    @State var libraryText = ""
    @State var libraryItems: [[String: Any]] = []
    @State var libraryLoaded = false

    // Bound root metadata depth: the device crash stack exhausted the main stack
    // resolving ContentView.body before its first frame. Computed `some View`
    // properties alone do not erase their underlying generic types.
    private var tabs: AnyView {
        AnyView(TabView(selection: $selectedTab) {
                AnyView(NavigationStack {
                    screen("Home")
                }.modifier(LegacyPlayerInset(open: { sheet = true }))).tabItem { Label("Home", systemImage: "house.fill") }.tag("Home")
                AnyView(NavigationStack {
                    searchScreen
                }.modifier(LegacyPlayerInset(open: { sheet = true }))).tabItem { Label("Search", systemImage: "magnifyingglass") }.tag("Search")
                AnyView(NavigationStack {
                    libraryList.navigationTitle("Your Library")
                }.modifier(LegacyPlayerInset(open: { sheet = true }))).tabItem { Label("Library", systemImage: "books.vertical") }.tag("Library")
                    .badge(music.attentionCount)
                AnyView(NavigationStack {
                    NativeSettings { screen("Downloads") }
                }.modifier(LegacyPlayerInset(open: { sheet = true }))).tabItem { Label("Settings", systemImage: "gear") }.tag("Settings")
            }
            .modifier(SystemTabPlayer(open: { sheet = true }))
        )
    }

    private var observedTabs: AnyView {
        AnyView(tabs
            .task {
                // Server failures change without the app's help; re-read them about once a minute.
                while !Task.isCancelled {
                    await music.checkServerAttention()
                    try? await Task.sleep(for: .seconds(60))
                }
            }
            .onChange(of: selectedTab) { previous, next in
                tabCatalogs[previous] = music.captureCatalog()
                if let snapshot = tabCatalogs[next] {
                    music.restoreCatalog(snapshot)
                } else {
                    music.restoreCatalog(["page": next, "title": next, "kind": "songs", "rows": []])
                }
                selecting = false; selectedIds = []
                if let pending = pendingNavigation {
                    pendingNavigation = nil
                    music.openCollection(pending.source, row: pending.row)
                }
            }
        )
    }

    private var collectionPresentations: AnyView {
        AnyView(observedTabs
            .sheet(isPresented: $collectionActions, onDismiss: {
                let action = pendingCollectionAction
                pendingCollectionAction = nil
                action?()
            }) { collectionActionsSheet }
            .sheet(item: $playlistSelection) { selection in PlaylistPickerSheet(selection: selection).environmentObject(music) }
            .sheet(item: $tagEditing) { selection in TagEditorSheet(song: selection.song).environmentObject(music) }
        )
    }

    private var playlistPresentations: AnyView {
        AnyView(collectionPresentations
            .alert(editingPlaylistId == nil ? "New playlist" : "Rename playlist", isPresented: $playlistEditor) {
                TextField("Playlist name", text: $playlistName)
                Button("Save") { let id = editingPlaylistId; let name = playlistName; Task { _ = await music.mutatePlaylist(["op": id == nil ? "create" : "rename", "id": id ?? "", "name": name], refresh: true) } }
                Button("Cancel", role: .cancel) {}
            }
            .confirmationDialog("Delete playlist?", isPresented: $deletingPlaylist, titleVisibility: .visible, presenting: deletingPlaylistId) { id in
                Button("Delete playlist", role: .destructive) { Task { _ = await music.mutatePlaylist(["op": "delete", "id": id], refresh: true) } }
            }
            .confirmationDialog("Remove all offline downloads?", isPresented: $clearDownloads, titleVisibility: .visible) {
                Button("Remove all downloads", role: .destructive) { music.downloads?.removeAll() }
            }
            .alert("Delete song from server?", isPresented: Binding(get: { pendingSongDeletion != nil }, set: { if !$0 { pendingSongDeletion = nil } }), presenting: pendingSongDeletion) { row in
                Button("Cancel", role: .cancel) { pendingSongDeletion = nil }
                Button("Delete", role: .destructive) {
                    pendingSongDeletion = nil
                    Task { await music.library("deleteSong", item: row, kind: "songs") }
                }
            } message: { row in
                Text("\u{201C}\(rowTitle(row))\u{201D} is removed from the server library for every device. Offline copies are managed separately.")
            }
        )
    }

    var body: some View {
        playlistPresentations
        .sheet(isPresented: $sheet) {
            NowPlayingView(goTo: goTo).environmentObject(music)
        }
    }

    /// Opens an album or artist from the player in place; Library and Settings roots are
    /// fixed menus, so those tabs hand the collection to Home.
    func goTo(_ source: String, _ row: [String: Any]) {
        sheet = false
        if ["Home", "Search"].contains(selectedTab) { music.openCollection(source, row: row) }
        else { pendingNavigation = (source, row); selectedTab = "Home" }
    }

    static let searchFilters: [(id: String, title: String)] = [("all", "All"), ("songs", "Songs"), ("albums", "Albums"), ("library", "Library")]

    var searchScreen: some View {
        VStack(spacing: 12) {
            if music.trail.isEmpty {
                HStack(spacing: 8) {
                    Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                    TextField("Search or paste a YouTube URL", text: $query)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .submitLabel(.search)
                        .focused($searchFocused)
                        .onSubmit { searchFocused = false; search() }
                    if !query.isEmpty {
                        Button { query = "" } label: { Image(systemName: "xmark.circle.fill").foregroundStyle(.secondary) }
                            .buttonStyle(.plain)
                            .accessibilityLabel("Clear search")
                    }
                }
                .padding(10)
                .background(Theme.elevated, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                .padding(.horizontal)
                Picker("Show", selection: Binding(get: { music.searchFilter }, set: { music.setSearchFilter($0) })) {
                    ForEach(Self.searchFilters, id: \.id) { filter in Text(filter.title).tag(filter.id) }
                }
                .pickerStyle(.segmented)
                .padding(.horizontal)
                .accessibilityIdentifier("search-filter")
                if !music.correction.isEmpty && music.correction.caseInsensitiveCompare(music.searchQuery) != .orderedSame {
                    Button { query = music.correction; search() } label: {
                        (Text("Did you mean ").foregroundStyle(.secondary) + Text(music.correction).bold().foregroundStyle(Theme.accent) + Text("?").foregroundStyle(.secondary))
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .buttonStyle(.plain)
                    .padding(.horizontal)
                    .accessibilityIdentifier("search-correction")
                }
                // Suggestions help only when the query found nothing.
                if music.rows.isEmpty && !music.loading && !music.searchQuery.isEmpty && !music.suggestions.isEmpty {
                    ScrollView(.horizontal) {
                        HStack {
                            ForEach(music.suggestions, id: \.self) { suggestion in
                                Button(suggestion) { query = suggestion; search() }.buttonStyle(.bordered)
                            }
                        }.padding(.horizontal)
                    }.scrollIndicators(.hidden)
                }
            }
            trackList
        }
        .navigationTitle(showsHero ? music.headerTitle : "Search")
        .navigationBarTitleDisplayMode(showsHero ? .inline : .large)
        .modifier(CatalogBackButton())
        .modifier(LoadCatalogOnFirstAppearance(page: "Omnibar"))
        .toolbar {
            if music.trail.isEmpty {
                ToolbarItem(placement: .topBarLeading) {
                    Menu {
                        Picker("Search in", selection: $music.searchMode) {
                            Label("Library and YouTube", systemImage: "globe").tag("Omnibar")
                            Label("Library only", systemImage: "books.vertical").tag("Search")
                            Label("New music only", systemImage: "sparkles").tag("New music")
                        }
                    } label: { Image(systemName: "line.3.horizontal.decrease.circle") }
                    .accessibilityLabel("Search source")
                    .accessibilityIdentifier("search-source")
                }
            }
            collectionToolbar
        }
        .onChange(of: music.searchMode) { _, _ in search() }
        // Search as you type, after a short pause, like Spotify.
        .onChange(of: query) { _, _ in
            searchTask?.cancel()
            searchTask = Task {
                do { try await Task.sleep(nanoseconds: 250_000_000) } catch { return }
                // Submitting or tapping a suggestion already searched for this text.
                if music.searchQuery == query && Music.searchPages.contains(music.page) { return }
                search()
            }
        }
    }

    func search() {
        searchTask?.cancel()
        selecting = false; selectedIds = []
        let mode = music.searchMode
        let text = query
        Task { await music.load(mode, query: text) }
    }

    /// Resolved playlist/album links: pick tracks and add them to a playlist in one go.
    @ViewBuilder var resolvedHeader: some View {
        if !music.resolvedKind.isEmpty && music.trail.isEmpty {
            let tracks = music.playable
            VStack(alignment: .leading, spacing: 8) {
                Text(music.resolvedTitle.isEmpty ? (music.resolvedKind == "album" ? "YouTube album" : "YouTube playlist") : music.resolvedTitle)
                    .font(.headline)
                Text("\(tracks.count) track\(tracks.count == 1 ? "" : "s") from YouTube").font(.subheadline).foregroundStyle(.secondary)
                HStack(spacing: 12) {
                    if selecting {
                        Button(selectedIds.count == tracks.count ? "Select none" : "Select all") {
                            selectedIds = selectedIds.count == tracks.count ? [] : Set(tracks.compactMap { $0["id"] as? String })
                        }
                        Button("Add \(selectedIds.count) to playlist") {
                            let chosen = tracks.filter { selectedIds.contains($0["id"] as? String ?? "") }
                            playlistSelection = PlaylistSelection(songs: chosen)
                        }
                        .disabled(selectedIds.isEmpty)
                        .accessibilityIdentifier("resolved-add-selected")
                        Spacer()
                        Button("Done") { selecting = false; selectedIds = [] }
                    } else {
                        Button { selecting = true; selectedIds = Set(tracks.compactMap { $0["id"] as? String }) } label: {
                            Label("Select songs", systemImage: "checkmark.circle")
                        }
                        .accessibilityIdentifier("resolved-select")
                        Button { playlistSelection = PlaylistSelection(songs: tracks) } label: {
                            Label("Add all to playlist", systemImage: "text.badge.plus")
                        }
                    }
                }
                .buttonStyle(.bordered)
                .font(.subheadline)
                .disabled(tracks.isEmpty)
            }
            .listRowSeparator(.hidden)
        }
    }

    static let libraryFilters: [(id: String, title: String)] = [("all", "All"), ("playlists", "Playlists"), ("albums", "Albums"), ("artists", "Artists")]
    static let librarySorts: [(id: String, title: String)] = [("recent", "Recently played"), ("added", "Recently added"), ("name", "Alphabetical")]

    var libraryList: some View {
        List {
            // Searching Your Library shows only matching items, like Spotify.
            if libraryText.isEmpty {
                Section {
                    libraryLink("Liked Songs", "Pinned playlist", kind: .liked)
                    libraryLink("Downloads", "Available offline", symbol: "arrow.down.circle.fill")
                    libraryLink("Queue", "What plays next", symbol: "list.number")
                }
                Section("Collections") {
                    libraryLink("Playlists", "Your playlists", kind: .playlist)
                    libraryLink("Albums", "Saved albums", kind: .album)
                    libraryLink("Artists", "Followed artists", kind: .artist)
                }
                Section("Browse") {
                    libraryLink("All Albums", "Every album in the library", symbol: "square.stack.fill")
                    libraryLink("All Artists", "Every artist in the library", symbol: "person.2.fill")
                    libraryLink("Songs", "Every song in the library", kind: .song)
                    libraryLink("Genres", "Browse by genre", kind: .genre)
                    libraryLink("Recently Added", "Newest albums", symbol: "sparkles")
                    libraryLink("Recently Played", "Albums you played lately", symbol: "clock.arrow.circlepath")
                    libraryLink("Recent Songs", "Your listening history on this device", symbol: "clock.fill")
                    libraryLink("Recent Collections", "Albums and playlists you played", symbol: "rectangle.stack.badge.play")
                }
                Section("ZayFetch") {
                    libraryLink("Jobs", "Downloads and imports in progress", symbol: "arrow.down.app.fill")
                        .badge(music.attentionCount)
                    libraryLink("Imports", "YouTube playlist imports", symbol: "square.and.arrow.down.fill")
                    libraryLink("YouTube Playlists", "Import from YouTube Music", symbol: "arrow.down.to.line")
                }
            }
            Section {
                // The pinned Liked Songs link above stands in for the core's pinned row.
                ForEach(Array(libraryItems.enumerated()).filter { libraryText.isEmpty ? $0.element["browseKind"] as? String != "liked" : true }, id: \.offset) { _, item in
                    libraryItemLink(item)
                }
                if libraryLoaded && libraryItems.isEmpty {
                    Text(libraryText.isEmpty ? "Saved playlists, albums and artists appear here." : "No matches in Your Library")
                        .foregroundStyle(.secondary).listRowSeparator(.hidden)
                }
            } header: {
                HStack {
                    Text(libraryText.isEmpty ? "All in Your Library" : "Results")
                    Spacer()
                    Menu {
                        Picker("Show", selection: $libraryFilter) {
                            ForEach(Self.libraryFilters, id: \.id) { Text($0.title).tag($0.id) }
                        }
                        Picker("Sort by", selection: $librarySort) {
                            ForEach(Self.librarySorts, id: \.id) { Text($0.title).tag($0.id) }
                        }
                    } label: {
                        Label(Self.librarySorts.first { $0.id == librarySort }?.title ?? "Sort", systemImage: "arrow.up.arrow.down")
                            .font(.footnote)
                    }
                    .accessibilityLabel("Filter and sort Your Library")
                    .accessibilityIdentifier("library-sort")
                }
                .textCase(nil)
            }
        }
        .listStyle(.plain)
        .searchable(text: $libraryText, placement: .navigationBarDrawer(displayMode: .automatic), prompt: "Search Your Library")
        .task(id: "\(libraryFilter)|\(librarySort)|\(libraryText)") {
            if !libraryText.isEmpty { do { try await Task.sleep(nanoseconds: 250_000_000) } catch { return } }
            await loadLibraryItems()
        }
    }

    /// The core's unified Library page: playlists, albums and artists with Liked Songs pinned.
    func loadLibraryItems() async {
        guard let core = music.core else { return }
        let query = encodeJSON(["filter": libraryFilter, "sort": librarySort, "text": libraryText])
        let text = await Task.detached { try? core.browse(page: "Library", query: query) }.value
        guard !Task.isCancelled else { return }
        libraryItems = text.map { decodeJSON($0)["rows"] as? [[String: Any]] ?? [] } ?? []
        libraryLoaded = text != nil
    }

    func libraryItemLink(_ item: [String: Any]) -> some View {
        let source = item["browseKind"] as? String ?? item["kind"] as? String ?? ""
        return NavigationLink {
            if source == "liked" { screen("Liked Songs") } else { collectionScreen(source, item) }
        } label: {
            HStack(spacing: 12) {
                ArtworkView(item: source == "liked" ? ["name": "Liked Songs"] : item, kind: ArtKind(catalogKind: source), size: 52)
                VStack(alignment: .leading, spacing: 2) {
                    Text(rowTitle(item)).font(.body.weight(.medium)).lineLimit(1)
                    Text(subtitle(item, kind: source)).font(.subheadline).foregroundStyle(.secondary).lineLimit(1)
                }
            }
            .padding(.vertical, 2)
        }
        .listRowSeparator(.hidden)
        .accessibilityIdentifier("library-item-\(item["id"] as? String ?? "unknown")")
    }

    /// A collection pushed by the system navigation stack; its own Back pops the stack.
    func collectionScreen(_ source: String, _ row: [String: Any]) -> some View {
        trackList
            .navigationTitle(music.headerTitle)
            .navigationBarTitleDisplayMode(.inline)
            .modifier(OpenCollectionOnFirstAppearance(source: source, row: row))
            .modifier(CatalogBackButton())
            .toolbar { collectionToolbar }
    }

    func libraryLink(_ page: String, _ subtitle: String, kind: ArtKind = .other, symbol: String? = nil) -> some View {
        NavigationLink { screen(page) } label: {
            LibraryRow(title: page, subtitle: subtitle, kind: kind, symbol: symbol)
        }
        .listRowSeparator(.hidden)
    }

    func screen(_ page: String) -> some View {
        trackList
            .navigationTitle(music.page == page ? page : music.headerTitle)
            .navigationBarTitleDisplayMode(showsHero ? .inline : .large)
            .modifier(LoadCatalogOnFirstAppearance(page: page))
            .modifier(CatalogBackButton())
            .task(id: music.page) {
                // Follows in-place navigation too, e.g. Imports → Show failed jobs.
                let current = music.page
                guard ["Jobs", "Imports", "YouTube Playlists"].contains(current) else { return }
                await music.checkConnection()
                guard current != "YouTube Playlists" else { return }
                while !Task.isCancelled && music.page == current {
                    do { try await Task.sleep(nanoseconds: 3_000_000_000) } catch { break }
                    if !Task.isCancelled && music.page == current { await music.refreshCatalog() }
                }
            }
            .toolbar { collectionToolbar }
    }

    var isPlaylistCollection: Bool { music.page == "Collection" && music.collectionContext["kind"] as? String == "playlists" }
    var showsCollectionActions: Bool {
        music.page == "Playlists" || music.page == "Downloads" || isPlaylistCollection || !music.playable.isEmpty
    }

    @ToolbarContentBuilder var collectionToolbar: some ToolbarContent {
        if showsCollectionActions {
            ToolbarItem(placement: .topBarTrailing) {
                Button { collectionActions = true } label: {
                    Image(systemName: "ellipsis").frame(width: 44, height: 44)
                }
                .accessibilityLabel("Collection actions")
                .accessibilityIdentifier("collection-actions")
            }
        }
    }

    func chooseCollectionAction(_ action: @escaping () -> Void) {
        pendingCollectionAction = action
        collectionActions = false
    }

    var collectionActionsSheet: some View {
        NavigationStack {
            List {
                if music.page == "Playlists" {
                    Button { chooseCollectionAction { editingPlaylistId = nil; playlistName = ""; playlistEditor = true } } label: { Label("New playlist", systemImage: "plus") }
                }
                if isPlaylistCollection {
                    Button { chooseCollectionAction { Task { await music.load("Playlists") } } } label: { Label("All playlists", systemImage: "music.note.list") }
                    Button {
                        let id = music.collectionContext["id"] as? String
                        let name = music.headerTitle
                        chooseCollectionAction { editingPlaylistId = id; playlistName = name; playlistEditor = true }
                    } label: { Label("Rename playlist", systemImage: "pencil") }
                    Button(role: .destructive) {
                        let id = music.collectionContext["id"] as? String ?? ""
                        chooseCollectionAction { deletingPlaylistId = id; deletingPlaylist = true }
                    } label: { Label("Delete playlist", systemImage: "trash") }
                }
                Button { chooseCollectionAction { music.downloadShown() } } label: { Label("Download all", systemImage: "arrow.down.circle") }
                    .disabled(music.downloadable.isEmpty)
                if music.page == "Downloads" {
                    Button(role: .destructive) { chooseCollectionAction { clearDownloads = true } } label: { Label("Remove all downloads", systemImage: "trash") }
                }
                Section("Shuffle") {
                    Button { chooseCollectionAction { music.shuffle("off") } } label: { Label("Shuffle songs", systemImage: "shuffle") }
                    Button { chooseCollectionAction { music.shuffle("sparse") } } label: { Label("Fresh songs · sparse", systemImage: "sparkles") }
                    Button { chooseCollectionAction { music.shuffle("dense") } } label: { Label("Fresh songs · dense", systemImage: "sparkles") }
                }.disabled(music.playable.isEmpty)
            }
            .navigationTitle("Collection actions")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { Button("Cancel") { collectionActions = false } }
        }.presentationDetents([.medium, .large])
    }

    /// Collections and Liked Songs get a Spotify-style header; other pages keep the large title.
    var showsHero: Bool { music.page == "Collection" || music.page == "Liked Songs" }
    var heroSource: String { music.page == "Liked Songs" ? "liked" : music.collectionContext["kind"] as? String ?? "" }
    var heroSaved: Bool { present(music.collectionContext["starred"]) }

    var heroMeta: String {
        let context = music.collectionContext
        var parts: [String] = []
        for key in ["owner", "artist"] {
            if let value = context[key] as? String, !value.isEmpty, value != music.headerTitle { parts.append(value) }
        }
        if let year = context["year"] as? Int, year > 0 { parts.append(String(year)) }
        let count = music.rows.count
        let noun = music.kind == "albums" ? "album" : "song"
        var total = "\(count) \(noun)\(count == 1 ? "" : "s")"
        let seconds = (context["duration"] as? NSNumber)?.doubleValue ?? 0
        if music.kind == "songs", seconds > 0 { total += ", \(Int((seconds / 60).rounded())) min" }
        parts.append(total)
        return parts.joined(separator: " · ")
    }

    var hero: some View {
        let context = music.collectionContext
        let source = heroSource
        return VStack(spacing: 14) {
            ArtworkView(item: source == "liked" ? ["name": "Liked Songs"] : context, kind: source == "liked" ? .liked : ArtKind(catalogKind: source), size: 220)
                .shadow(color: .black.opacity(0.5), radius: 20, y: 10)
            VStack(spacing: 4) {
                Text(music.headerTitle).font(.title2.bold()).multilineTextAlignment(.center)
                Text(heroMeta).font(.subheadline).foregroundStyle(.secondary)
            }
            HStack(spacing: 20) {
                if ["albums", "playlists", "genres", "liked"].contains(source) {
                    Button { music.downloadShown() } label: {
                        Image(systemName: "arrow.down.circle").font(.title2)
                    }
                    .accessibilityLabel("Download all")
                    .disabled(music.downloadable.isEmpty)
                }
                if source == "albums" {
                    Button { Task { await music.library(heroSaved ? "unsave" : "save", item: context, kind: "albums") } } label: {
                        Image(systemName: heroSaved ? "checkmark.circle.fill" : "plus.circle").font(.title2)
                            .foregroundStyle(heroSaved ? Theme.accent : Color.white)
                    }
                    .accessibilityLabel(heroSaved ? "Remove from Your Library" : "Save to Your Library")
                    .accessibilityIdentifier("album-save")
                }
                if source == "artists" {
                    Button(heroSaved ? "Following" : "Follow") {
                        Task { await music.library(heroSaved ? "unsave" : "save", item: context, kind: "artists") }
                    }
                    .buttonStyle(.bordered)
                    .buttonBorderShape(.capsule)
                    .accessibilityHint(heroSaved ? "Unfollows this artist" : "Adds this artist to Your Library")
                    .accessibilityIdentifier("artist-follow")
                }
                if source == "newAlbums" {
                    Button { Task { await music.library("acquireAlbum", item: context, kind: "newAlbums") } } label: {
                        Label("Add to library", systemImage: "plus.circle")
                    }
                    .buttonStyle(.bordered)
                    .buttonBorderShape(.capsule)
                    .accessibilityIdentifier("album-acquire")
                    Button { playlistSelection = PlaylistSelection(album: context) } label: {
                        Image(systemName: "text.badge.plus").font(.title2)
                    }
                    .accessibilityLabel("Add album to playlist")
                    .accessibilityIdentifier("album-acquire-playlist")
                }
                Spacer()
                if music.kind == "songs" {
                    Button { music.shuffle("off") } label: {
                        Image(systemName: "shuffle").font(.title2)
                    }
                    .accessibilityLabel("Shuffle play")
                    .disabled(music.playable.isEmpty)
                    // Playlists and Liked Songs start shuffled; albums play from the first track.
                    Button { music.playCollection() } label: {
                        Image(systemName: "play.fill")
                            .font(.title2)
                            .foregroundStyle(.black)
                            .frame(width: 56, height: 56)
                            .background(Theme.accent, in: Circle())
                    }
                    .accessibilityLabel("Play \(music.headerTitle)")
                    .disabled(music.playable.isEmpty)
                }
            }
            .buttonStyle(.borderless)
        }
        .frame(maxWidth: .infinity)
        .padding(.top, 8)
        .listRowSeparator(.hidden)
        .listRowInsets(EdgeInsets(top: 8, leading: 16, bottom: 8, trailing: 16))
    }

    /// Spotify-style Home shortcuts to the collections played most recently.
    var shortcuts: some View {
        LazyVGrid(columns: [GridItem(.flexible(), spacing: 8), GridItem(.flexible(), spacing: 8)], spacing: 8) {
            ForEach(Array(music.recentContexts.prefix(8).enumerated()), id: \.offset) { _, item in
                let source = item["kind"] as? String ?? ""
                let id = item["id"] as? String ?? "unknown"
                HStack(spacing: 0) {
                    Button { var row = item; row["browseKind"] = source; music.select(row) } label: {
                        HStack(spacing: 8) {
                            ArtworkView(item: source == "liked" ? ["name": "Liked Songs"] : item, kind: source == "liked" ? .liked : ArtKind(catalogKind: source), size: 56)
                            Text(rowTitle(item)).font(.footnote.weight(.semibold)).lineLimit(2).multilineTextAlignment(.leading)
                            Spacer(minLength: 0)
                        }
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(rowTitle(item))
                    .accessibilityIdentifier("shortcut-\(id)")
                    Button { music.playShortcut(item) } label: {
                        Image(systemName: "play.fill").font(.caption.weight(.bold)).foregroundStyle(.black)
                            .frame(width: 28, height: 28).background(Theme.accent, in: Circle())
                            .frame(width: 40, height: 56).contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Play \(rowTitle(item))")
                    .accessibilityIdentifier("shortcut-play-\(id)")
                }
                .background(Theme.elevated, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                .clipShape(RoundedRectangle(cornerRadius: 6, style: .continuous))
            }
        }
        .listRowSeparator(.hidden)
        .listRowInsets(EdgeInsets(top: 4, leading: 16, bottom: 8, trailing: 16))
    }

    /// Home's server sections and the queue's lanes; otherwise one untitled section.
    var displaySections: [(title: String, range: Range<Int>)] {
        let count = music.rows.count
        if music.page == "Queue" {
            let priority = music.rows.filter { $0["queueLane"] as? String == "priority" }.count
            let lanes: [(title: String, range: Range<Int>)] = [("Next in queue", 0..<priority), ("Main queue", priority..<count)]
            return lanes.filter { !$0.range.isEmpty }
        }
        let parsed: [(title: String, range: Range<Int>)] = music.sections.compactMap { section in
            guard let title = section["title"] as? String, let start = section["start"] as? Int, let length = section["count"] as? Int,
                  start >= 0, length >= 0, start + length <= count else { return nil }
            return (title: title, range: start..<(start + length))
        }
        return parsed.isEmpty ? [(title: "", range: 0..<count)] : parsed
    }

    static let jobFilters: [(id: String, title: String)] = [("active", "In progress"), ("done", "Completed"), ("attention", "Needs attention")]

    /// Counts double as filters: tap one to show only those jobs, tap again for all.
    @ViewBuilder var jobSummary: some View {
        if music.kind == "jobs" && !music.rows.isEmpty {
            let buckets = music.rows.map { jobBucket($0["status"] as? String ?? "") }
            ScrollView(.horizontal) {
                HStack(spacing: 8) {
                    ForEach(Self.jobFilters, id: \.id) { filter in
                        let count = buckets.filter { $0 == filter.id }.count
                        let selected = music.jobFilter == filter.id
                        Button { music.jobFilter = selected ? "all" : filter.id } label: {
                            Text("\(filter.title) · \(count)").font(.subheadline)
                        }
                        .buttonStyle(.bordered)
                        .buttonBorderShape(.capsule)
                        .tint(selected ? Theme.accent : (filter.id == "attention" && count > 0 ? .orange : .secondary))
                        .accessibilityAddTraits(selected ? .isSelected : [])
                        .accessibilityIdentifier("job-filter-\(filter.id)")
                    }
                }
            }
            .scrollIndicators(.hidden)
            .listRowSeparator(.hidden)
        }
    }
    /// Rows a section shows: job filters hide others; everything else shows its whole range.
    func visibleIndices(_ range: Range<Int>) -> [Int] {
        let indices = range.filter { $0 < music.rows.count }
        guard music.kind == "jobs", music.jobFilter != "all" else { return indices }
        return indices.filter { jobBucket(music.rows[$0]["status"] as? String ?? "") == music.jobFilter }
    }

    @ViewBuilder var statusRows: some View {
        if ["Jobs", "Imports", "YouTube Playlists"].contains(music.page) && !music.connectionWarning.isEmpty {
            Button { selectedTab = "Settings" } label: {
                Label { Text(music.connectionWarning).multilineTextAlignment(.leading) } icon: { Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.orange) }
                    .font(.subheadline)
            }
            .listRowSeparator(.hidden)
            .accessibilityHint("Opens Settings")
            .accessibilityIdentifier("connection-warning")
        }
        if music.page == "Downloads" {
            Label("\(ByteCountFormatter.string(fromByteCount: Int64(music.storageBytes), countStyle: .file)) stored on this iPhone", systemImage: "internaldrive")
                .font(.subheadline).foregroundStyle(.secondary).listRowSeparator(.hidden)
        }
        if !music.notice.isEmpty {
            Label { Text(music.notice) } icon: { Image(systemName: "checkmark.circle.fill").foregroundStyle(Theme.accent) }
                .font(.subheadline).listRowSeparator(.hidden)
                .accessibilityIdentifier("catalog-notice")
        }
        if music.offline {
            Label("Offline · showing saved library", systemImage: "icloud.slash").font(.subheadline).foregroundStyle(.orange).listRowSeparator(.hidden)
        }
        if !music.error.isEmpty {
            Label(music.error, systemImage: "exclamationmark.triangle.fill").font(.subheadline).foregroundStyle(.orange).listRowSeparator(.hidden)
                .accessibilityIdentifier("catalog-error")
        }
    }

    var trackList: some View {
        List {
            if showsHero { hero }
            if music.page == "Home" && !music.recentContexts.isEmpty { shortcuts }
            if Music.searchPages.contains(music.page) { resolvedHeader }
            statusRows
            jobSummary
            if showsHero && music.loading && music.rows.isEmpty {
                ProgressView().frame(maxWidth: .infinity).listRowSeparator(.hidden)
            }
            ForEach(Array(displaySections.enumerated()), id: \.offset) { _, section in
                Section {
                    ForEach(visibleIndices(section.range), id: \.self) { index in
                        if ["jobs", "imports", "youtubePlaylists"].contains(music.kind) {
                            ManagementRow(item: music.rows[index], kind: music.kind)
                        } else { trackRow(music.rows[index], at: index) }
                    }
                } header: {
                    if !section.title.isEmpty {
                        Text(section.title).font(.title3.bold()).foregroundStyle(.primary).textCase(nil)
                    }
                }
            }
        }
        .listStyle(.plain)
        .refreshable { await music.refreshCatalog() }
        .overlay {
            if music.rows.isEmpty && !showsHero && music.error.isEmpty && !(music.page == "Home" && !music.recentContexts.isEmpty) {
                if music.loading {
                    ProgressView()
                } else if Music.searchPages.contains(music.page) {
                    if music.searchQuery.isEmpty {
                        ContentUnavailableView("Search your music", systemImage: "magnifyingglass", description: Text("Find songs, albums and artists, or paste a YouTube link."))
                    } else if music.searchFilter != "all" && !music.searchSourceRows.isEmpty {
                        ContentUnavailableView("No \(Self.searchFilters.first { $0.id == music.searchFilter }?.title.lowercased() ?? "results") for \u{201C}\(music.searchQuery)\u{201D}", systemImage: "line.3.horizontal.decrease.circle", description: Text("Try another filter."))
                    } else {
                        ContentUnavailableView.search(text: music.searchQuery)
                    }
                } else {
                    ContentUnavailableView("Nothing here yet", systemImage: "music.note.list")
                }
            }
        }
    }

    func isCurrent(_ row: [String: Any]) -> Bool {
        guard music.rowKind(row) == "songs", let id = row["id"] as? String else { return false }
        return id == music.currentSong["id"] as? String
    }

    func subtitle(_ row: [String: Any], kind: String? = nil) -> String {
        let count = row["songCount"] as? Int
        switch kind ?? music.rowKind(row) {
        case "albums", "newAlbums":
            return ["Album", row["artist"] as? String ?? ""].filter { !$0.isEmpty }.joined(separator: " · ")
        case "artists":
            return "Artist"
        case "playlists":
            return "Playlist" + (count.map { " · \($0) song\($0 == 1 ? "" : "s")" } ?? "")
        case "liked":
            return "Playlist"
        case "genres":
            return count.map { "\($0) song\($0 == 1 ? "" : "s")" } ?? "Genre"
        default:
            let lane = row["queueLane"] as? String
            return [lane == "priority" ? "Next in queue" : lane != nil ? "Main queue" : "", row["artist"] as? String ?? ""].filter { !$0.isEmpty }.joined(separator: " · ")
        }
    }

    func trackRow(_ row: [String: Any], at index: Int) -> some View {
        let download = row["download"] as? [String: Any]
        let state = download?["state"] as? String ?? ""
        let source = music.rowKind(row)
        let id = row["id"] as? String ?? "unknown"
        let choosing = selecting && !music.resolvedKind.isEmpty && source == "songs"
        return Button {
            if choosing { if selectedIds.contains(id) { selectedIds.remove(id) } else { selectedIds.insert(id) } }
            else { music.select(row, at: index) }
        } label: {
            HStack(spacing: 12) {
                if choosing {
                    Image(systemName: selectedIds.contains(id) ? "checkmark.circle.fill" : "circle")
                        .font(.title3).foregroundStyle(selectedIds.contains(id) ? Theme.accent : Color.secondary)
                }
                ArtworkView(item: source == "liked" ? ["name": "Liked Songs"] : row, kind: source == "liked" ? .liked : ArtKind(catalogKind: source), size: 48)
                VStack(alignment: .leading, spacing: 3) {
                    Text(rowTitle(row))
                        .font(.body)
                        .foregroundStyle(isCurrent(row) ? Theme.accent : Color.primary)
                        .lineLimit(1)
                    HStack(spacing: 5) {
                        if source == "songs" && (row["id"] as? String)?.hasPrefix("yt:") == true {
                            Text("PREVIEW")
                                .font(.caption2.weight(.bold))
                                .padding(.horizontal, 4)
                                .padding(.vertical, 1)
                                .background(Color.white.opacity(0.15), in: RoundedRectangle(cornerRadius: 3))
                        }
                        if state == "ready" {
                            Image(systemName: "arrow.down.circle.fill").font(.caption).foregroundStyle(Theme.accent).accessibilityLabel("Downloaded")
                        }
                        Text(subtitle(row)).font(.subheadline).foregroundStyle(.secondary).lineLimit(1)
                    }
                }
                Spacer(minLength: 8)
                if let download, state != "ready" {
                    VStack(alignment: .trailing, spacing: 3) {
                        Text(state.capitalized).font(.caption)
                        if let value = download["progress"] as? Double, state == "downloading" || state == "queued" { ProgressView(value: value).tint(Theme.accent).frame(width: 54) }
                        if let message = download["error"] as? String { Text(message).font(.caption2).foregroundStyle(.orange) }
                    }.foregroundStyle(.secondary).frame(maxWidth: 140, alignment: .trailing)
                }
                if source != "songs" {
                    Image(systemName: "chevron.forward").font(.footnote.weight(.semibold)).foregroundStyle(.tertiary)
                }
            }
            .contentShape(Rectangle())
            // Install the menu on the label hit area so a long press is handled
            // before the outer playback button and List swipe actions.
            .contextMenu { rowMenu(row, source: source, download: download, state: state) }
        }
        .buttonStyle(.plain)
        .listRowSeparator(.hidden)
        .listRowInsets(EdgeInsets(top: 6, leading: 16, bottom: 6, trailing: 16))
        .accessibilityIdentifier("row-\(id)")
        .accessibilityAddTraits(choosing && selectedIds.contains(id) ? .isSelected : [])
        .accessibilityValue(row["playlistIndex"].map { "Playlist position \(($0 as? Int ?? 0) + 1)" } ?? "")
        .swipeActions(edge: .leading) {
            if source == "songs" && row["queueLane"] == nil {
                Button { music.action("playNext", row) } label: { Label("Play next", systemImage: "text.line.first.and.arrowtriangle.forward") }.tint(Theme.accent)
            }
        }
        .swipeActions(edge: .trailing) {
            if row["queueLane"] != nil {
                Button(role: .destructive) { music.queueAction("remove", row: row) } label: { Label("Remove from queue", systemImage: "minus.circle") }
            }
        }
    }

    @ViewBuilder func rowMenu(_ row: [String: Any], source: String, download: [String: Any]?, state: String) -> some View {
        let id = row["id"] as? String ?? ""
        let preview = id.hasPrefix("yt:")
        if source == "songs" {
            Button { playlistSelection = PlaylistSelection(song: row) } label: { Label("Add to playlist", systemImage: "text.badge.plus") }
            if row["playlistId"] != nil {
                Button { Task { await music.editPlaylistTrack("move", row: row, delta: -1) } } label: { Label("Move up in playlist", systemImage: "arrow.up") }
                Button { Task { await music.editPlaylistTrack("move", row: row, delta: 1) } } label: { Label("Move down in playlist", systemImage: "arrow.down") }
                Button(role: .destructive) { Task { await music.editPlaylistTrack("remove", row: row) } } label: { Label("Remove from playlist", systemImage: "minus.circle") }
            }
            Button { music.action("playNext", row) } label: { Label("Play next", systemImage: "text.line.first.and.arrowtriangle.forward") }
            Button { music.action("playLast", row) } label: { Label("Play last in priority queue", systemImage: "text.line.last.and.arrowtriangle.forward") }
            if let album = albumLink(row) {
                Button { music.openCollection("albums", row: album) } label: { Label("Go to album", systemImage: "opticaldisc") }
            }
            if let artist = artistLink(row) {
                Button { music.openCollection("artists", row: artist) } label: { Label("Go to artist", systemImage: "person.crop.circle") }
            }
            Button { music.action("append", row) } label: { Label("Add to queue", systemImage: "text.append") }
            if row["queueLane"] != nil {
                Button { music.queueAction("move", row: row, delta: -1) } label: { Label("Move up", systemImage: "arrow.up") }
                Button { music.queueAction("move", row: row, delta: 1) } label: { Label("Move down", systemImage: "arrow.down") }
                Button(role: .destructive) { music.queueAction("remove", row: row) } label: { Label("Remove from queue", systemImage: "minus.circle") }
            }
            if preview {
                Button { music.action("acquire", row) } label: { Label("Add to library", systemImage: "plus.circle") }
            } else if !id.isEmpty {
                let liked = present(row["starred"]) || music.page == "Liked Songs"
                Button { Task { await music.library(liked ? "unsave" : "save", item: row, kind: "songs") } } label: {
                    Label(liked ? "Remove from Liked Songs" : "Add to Liked Songs", systemImage: liked ? "heart.slash" : "heart")
                }
                if download == nil {
                    Button { music.action("download", row) } label: { Label("Download", systemImage: "arrow.down.circle") }
                }
                Button { tagEditing = TagSelection(song: row) } label: { Label("Edit tags…", systemImage: "tag") }
            }
            if download != nil {
                if ["queued", "downloading"].contains(state) { Button { music.action("cancelDownload", row) } label: { Label("Cancel download", systemImage: "xmark.circle") } }
                if ["failed", "cancelled"].contains(state) { Button { music.action("download", row) } label: { Label("Retry download", systemImage: "arrow.clockwise") } }
                Button(role: .destructive) { music.action("removeDownload", row) } label: { Label("Remove download", systemImage: "trash") }
            }
            if !preview && !id.isEmpty {
                Button(role: .destructive) { pendingSongDeletion = row } label: { Label("Delete from server…", systemImage: "trash") }
            }
        } else if source == "albums" || source == "artists" {
            // Saved-album and followed-artist lists contain only saved items.
            let saved = present(row["starred"]) || music.page == (source == "albums" ? "Albums" : "Artists")
            Button { Task { await music.library(saved ? "unsave" : "save", item: row, kind: source) } } label: {
                if source == "albums" {
                    Label(saved ? "Remove from Your Library" : "Save to Your Library", systemImage: saved ? "minus.circle" : "plus.circle")
                } else {
                    Label(saved ? "Unfollow" : "Follow", systemImage: saved ? "person.badge.minus" : "person.badge.plus")
                }
            }
            // Artists have no per-song download; open one of their albums instead.
            if source == "albums" {
                Button { music.action("download", row) } label: { Label("Download album", systemImage: "arrow.down.circle") }
            }
        } else if source == "newAlbums" {
            Button { Task { await music.library("acquireAlbum", item: row, kind: "newAlbums") } } label: { Label("Add album to library", systemImage: "plus.circle") }
        } else if source == "playlists" {
            Button { music.action("download", row) } label: { Label("Download playlist", systemImage: "arrow.down.circle") }
            if music.kind == "playlists" {
                Button { editingPlaylistId = row["id"] as? String; playlistName = row["name"] as? String ?? ""; playlistEditor = true } label: { Label("Rename playlist", systemImage: "pencil") }
                Button(role: .destructive) { deletingPlaylistId = row["id"] as? String ?? ""; deletingPlaylist = true } label: { Label("Delete playlist", systemImage: "trash") }
            }
        }
    }
}
