import AVFoundation
import Combine
import XCTest
@testable import ZayMusic
final class CoreTests: XCTestCase {
    func testSharedQueueAcrossSwiftBoundary() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let core = try NativeCore(server: "https://example.invalid", directory: root.path)
        let result = try core.queueCommand(command: "{\"op\":\"set\",\"songs\":[{\"id\":\"fake\",\"title\":\"Fixture\"}]}")
        XCTAssertEqual((decodeJSON(result)["current"] as? [String: Any])?["id"] as? String, "fake")
        let stopped = decodeJSON(try core.queueCommand(command: "{\"op\":\"next\"}"))
        XCTAssertEqual(stopped["continue"] as? Bool, false)
    }
    func testPreviousRestartsAfterThreeSeconds() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let core = try NativeCore(server: "https://example.invalid", directory: root.path)
        _ = try core.queueCommand(command: encodeJSON(["op": "set", "songs": [["id": "a", "title": "A"], ["id": "b", "title": "B"]], "index": 1]))
        let restart = decodeJSON(try core.queueCommand(command: encodeJSON(["op": "previous", "position": 5.0])))
        XCTAssertEqual(restart["restart"] as? Bool, true)
        XCTAssertEqual((restart["current"] as? [String: Any])?["id"] as? String, "b", "Restarting keeps the song")
        let back = decodeJSON(try core.queueCommand(command: encodeJSON(["op": "previous", "position": 1.0])))
        XCTAssertNotEqual(back["restart"] as? Bool, true)
        XCTAssertEqual((back["current"] as? [String: Any])?["id"] as? String, "a")
    }
    func testSavedPositionResumesOnlyTheSameTrack() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let core = try NativeCore(server: "https://example.invalid", directory: root.path)
        let snapshot = decodeJSON(try core.queueCommand(command: encodeJSON(["op": "set", "songs": [["id": "a", "title": "A"], ["id": "b", "title": "B"]]])))
        let epoch = try XCTUnwrap(snapshot["playbackEpoch"] as? NSNumber).uint64Value
        try core.savePlaybackPosition(epoch: epoch, songId: "a", position: 42)
        try core.savePlaybackPosition(epoch: epoch &+ 1, songId: "a", position: 7)
        try core.savePlaybackPosition(epoch: epoch, songId: "b", position: 9)
        XCTAssertEqual((decodeJSON(try core.queueCommand(command: "{}"))["resumePosition"] as? NSNumber)?.doubleValue, 42, "Stale reports are ignored")
        let relaunched = try NativeCore(server: "https://example.invalid", directory: root.path)
        XCTAssertEqual((decodeJSON(try relaunched.queueCommand(command: "{}"))["resumePosition"] as? NSNumber)?.doubleValue, 42, "Launch restores the position")
        let next = decodeJSON(try relaunched.queueCommand(command: "{\"op\":\"next\"}"))
        XCTAssertEqual((next["resumePosition"] as? NSNumber)?.doubleValue ?? 0, 0, "A new track starts from the beginning")
    }
}

@MainActor
final class NativePlaybackTests: XCTestCase {
    func testNativeQueueAutoAdvancesAcrossPreparedItems() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let core = try NativeCore(server: "https://example.invalid", directory: root.path)
        var songs: [[String: Any]] = []
        for (index, name) in ["A", "B", "C"].enumerated() {
            let path = root.appendingPathComponent("\(name).wav")
            try silentWav().write(to: path)
            songs.append(["id": name, "title": name, "duration": 0.25, "suffix": "wav", "replayGain": ["trackGain": -6.0 * Double(index)]])
        }
        for song in songs {
            let name = song["title"] as! String
            try core.registerDownload(songJson: encodeJSON(song), temporaryFile: root.appendingPathComponent("\(name).wav").path)
        }
        _ = try core.queueCommand(command: encodeJSON(["op": "set", "songs": songs]))
        try core.setAutoplay(enabled: false)
        let music = Music()
        music.core = core
        let ended = expectation(description: "All three native items finish")
        var began = false
        let observation = music.$playing.sink { value in
            if value { began = true }
            else if began { ended.fulfill() }
        }
        music.rebuild()
        await fulfillment(of: [ended], timeout: 15)
        let result = decodeJSON(try core.queueCommand(command: "{}"))
        let state = result["state"] as? [String: Any]
        XCTAssertEqual(state?["index"] as? Int, 2)
        XCTAssertEqual(music.current, "C")
        XCTAssertFalse(music.playing)
        // History must come from the actual AVQueuePlayer end events, including short items.
        var recents: [[String: Any]] = []
        for _ in 0..<50 {
            recents = try JSONSerialization.jsonObject(with: Data(core.recentSongs().utf8)) as! [[String: Any]]
            if recents.count == 3 { break }
            try await Task.sleep(nanoseconds: 20_000_000)
        }
        XCTAssertEqual(recents.compactMap { $0["id"] as? String }, ["C", "B", "A"])
        XCTAssertEqual(music.player.volume, Float(pow(10, -12.0 / 20)), accuracy: 0.001)
        withExtendedLifetime(observation) {}
    }
    private func silentWav() -> Data {
        let samples: UInt32 = 11025
        var data = Data()
        func text(_ value: String) { data.append(contentsOf: value.utf8) }
        func number<T: FixedWidthInteger>(_ value: T) {
            var little = value.littleEndian
            withUnsafeBytes(of: &little) { data.append(contentsOf: $0) }
        }
        text("RIFF"); number(UInt32(36 + samples * 2)); text("WAVEfmt ")
        number(UInt32(16)); number(UInt16(1)); number(UInt16(1))
        number(UInt32(44100)); number(UInt32(88200)); number(UInt16(2)); number(UInt16(16))
        text("data"); number(samples * 2); data.append(Data(repeating: 0, count: Int(samples) * 2))
        return data
    }
}

final class NativeHelperTests: XCTestCase {
    func testServerAddressNeverKeepsCredentials() {
        XCTAssertNil(validatedServerAddress("https://user:secret@example.invalid"))
        XCTAssertNil(validatedServerAddress("https://user@example.invalid"))
        XCTAssertNil(validatedServerAddress("ftp://example.invalid"))
        XCTAssertNil(validatedServerAddress("https://example.invalid/?token=fake"))
        XCTAssertNil(validatedServerAddress("example.invalid"))
        XCTAssertEqual(validatedServerAddress("  https://example.invalid:8443/base  "), "https://example.invalid:8443/base")
    }
    func testStoredCredentialedServerIsDiscarded() throws {
        let suite = UUID().uuidString
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        defaults.set("https://user:secret@example.invalid", forKey: "server")
        XCTAssertEqual(NativeEnvironment.configuredServer(defaults), defaultServerAddress)
        XCTAssertNil(defaults.string(forKey: "server"), "A credentialed address must not stay persisted")
        defaults.set("https://example.invalid", forKey: "server")
        XCTAssertEqual(NativeEnvironment.configuredServer(defaults), "https://example.invalid")
    }
    func testMixedRowsNeverBecomeSongsOrDownloads() {
        let rows: [[String: Any]] = [["id": "al", "browseKind": "albums"], ["id": "s1"], ["id": "ar", "browseKind": "artists"],
                                     ["id": "yt:v", "previewUrl": "/x"], ["id": "s2", "browseKind": "songs"], ["id": "na", "browseKind": "newAlbums"]]
        XCTAssertEqual(playableRows(rows).compactMap { $0["id"] as? String }, ["s1", "yt:v", "s2"])
        XCTAssertEqual(downloadableRows(rows).compactMap { $0["id"] as? String }, ["s1", "s2"])
        let context = playbackContext(rows, sections: [0..<3, 3..<6], selected: 4)
        XCTAssertEqual(context?.songs.compactMap { $0["id"] as? String }, ["yt:v", "s2"])
        XCTAssertEqual(context?.index, 1)
        XCTAssertNil(playbackContext(rows, sections: [], selected: 0), "Album rows are opened, not played")
        XCTAssertFalse(present(NSNull()), "JSON null must not read as a saved item")
    }
    func testSearchChipsKeepSectionsAndOrder() {
        let rows: [[String: Any]] = [["id": "s1"], ["id": "s2"], ["id": "al", "browseKind": "albums"], ["id": "ar", "browseKind": "artists"],
                                     ["id": "yt:v"], ["id": "na", "browseKind": "newAlbums"]]
        let sections: [[String: Any]] = [["title": "In your library", "start": 0, "count": 2], ["title": "Albums", "start": 2, "count": 1],
                                         ["title": "Artists", "start": 3, "count": 1], ["title": "New music", "start": 4, "count": 1], ["title": "New albums", "start": 5, "count": 1]]
        func ids(_ filter: String) -> [String] { filterSearch(rows, sections: sections, filter: filter).rows.compactMap { $0["id"] as? String } }
        XCTAssertEqual(ids("all").count, 6)
        XCTAssertEqual(ids("songs"), ["s1", "s2", "yt:v"])
        XCTAssertEqual(ids("albums"), ["al", "na"])
        XCTAssertEqual(ids("library"), ["s1", "s2", "al", "ar"])
        let albums = filterSearch(rows, sections: sections, filter: "albums").sections
        XCTAssertEqual(albums.compactMap { $0["title"] as? String }, ["Albums", "New albums"])
        XCTAssertEqual(albums.compactMap { $0["start"] as? Int }, [0, 1], "Filtered sections index the filtered rows")
        XCTAssertEqual(searchCorrection(["artist": "Fake Artist", "title": "Fake Song"]), "Fake Artist Fake Song")
        XCTAssertEqual(searchCorrection(" fake "), "fake")
    }
    func testPlaybackFailuresExplainOfflineAndPreviews() {
        let offline = NSError(domain: "AVFoundationErrorDomain", code: -11800, userInfo: [NSUnderlyingErrorKey: NSError(domain: NSURLErrorDomain, code: NSURLErrorNotConnectedToInternet)])
        XCTAssertTrue(isOfflineError(offline))
        XCTAssertFalse(isOfflineError(NSError(domain: NSURLErrorDomain, code: NSURLErrorBadServerResponse)))
        XCTAssertTrue(playbackFailureMessage(offline, preview: false, local: false).contains("isn't downloaded"))
        XCTAssertTrue(playbackFailureMessage(nil, preview: true, local: false).contains("preview"))
        XCTAssertTrue(playbackFailureMessage(offline, preview: false, local: true).contains("downloaded file"))
    }
    func testManagementAndNavigationHelpers() {
        XCTAssertEqual(["queued", "running", "done", "failed", "cancelled"].map(jobBucket), ["active", "active", "done", "attention", "attention"])
        XCTAssertFalse(youtubeAttention(["configured": false]).isEmpty)
        XCTAssertFalse(youtubeAttention(["configured": true, "verified": false]).isEmpty)
        XCTAssertTrue(youtubeAttention(["configured": true, "verified": true]).isEmpty)
        let song: [String: Any] = ["id": "s1", "album": "Fake album", "albumId": "al1", "artist": "Fake artist", "artistId": "ar1"]
        XCTAssertEqual(albumLink(song)?["id"] as? String, "al1")
        XCTAssertEqual(artistLink(song)?["name"] as? String, "Fake artist")
        XCTAssertNil(albumLink(["id": "yt:v", "albumId": "al1"]), "Previews have no library album")
        XCTAssertEqual(playContextValue(page: "Liked Songs", collection: [:])["kind"] as? String, "liked")
        XCTAssertTrue(playContextValue(page: "Home", collection: ["kind": "albums"]).isEmpty, "Home shelves have no collection")
        let edits = tagEdits(title: " Fake ", artist: "A", album: "B", track: "", year: "2020", genre: "Pop")
        XCTAssertEqual(edits["title"] as? String, "Fake")
        XCTAssertTrue(edits["track"] is NSNull, "A blank number clears the tag")
        XCTAssertEqual(edits["year"] as? Int, 2020)
    }
}

@MainActor
final class NativeCatalogTests: XCTestCase {
    private func makeMusic() throws -> (Music, NativeCore) {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: root) }
        let core = try NativeCore(server: "https://example.invalid", directory: root.path)
        try core.setAutoplay(enabled: false)
        let music = Music()
        music.core = core
        return (music, core)
    }
    private func queueIds(_ core: NativeCore) throws -> [String] {
        let state = decodeJSON(try core.queueCommand(command: "{}"))["state"] as? [String: Any]
        return (state?["queue"] as? [[String: Any]] ?? []).compactMap { $0["id"] as? String }
    }
    func testMixedSearchSelectionQueuesOnlyItsSongs() throws {
        let (music, core) = try makeMusic()
        music.page = "Omnibar"; music.kind = "songs"
        music.rows = [["id": "s1", "title": "One"], ["id": "s2", "title": "Two"],
                      ["id": "al", "title": "Album", "browseKind": "albums"], ["id": "ar", "title": "Artist", "browseKind": "artists"]]
        music.sections = [["title": "In your library", "start": 0, "count": 2], ["title": "Albums", "start": 2, "count": 1], ["title": "Artists", "start": 3, "count": 1]]
        music.select(music.rows[1], at: 1)
        XCTAssertEqual(try queueIds(core), ["s1", "s2"])
        XCTAssertEqual(music.currentSong["id"] as? String, "s2")
        XCTAssertEqual(music.downloadable.count, 2, "Album and artist rows must never be downloaded as songs")
        music.pause()
    }
    func testCollectionsOpenInPlaceAndBackRestoresResults() throws {
        let (music, _) = try makeMusic()
        music.page = "Omnibar"; music.kind = "songs"; music.searchQuery = "fixture"
        music.rows = [["id": "s1", "title": "One"], ["id": "ar", "title": "Artist", "browseKind": "artists"]]
        music.select(music.rows[1], at: 1)
        XCTAssertEqual(music.page, "Collection")
        XCTAssertEqual(music.headerTitle, "Artist")
        XCTAssertEqual(music.kind, "albums", "An artist page lists albums")
        XCTAssertEqual(music.trail.count, 1)
        XCTAssertEqual((music.captureCatalog()["trail"] as? [[String: Any]])?.count, 1, "Tabs keep their trail across switches")
        music.back()
        XCTAssertEqual(music.page, "Omnibar")
        XCTAssertEqual(music.searchQuery, "fixture")
        XCTAssertEqual(music.rows.count, 2)
        XCTAssertTrue(music.trail.isEmpty)
    }
    func testQueueEditsLeaveTheVisibleCatalogUntouched() throws {
        let (music, core) = try makeMusic()
        _ = try core.queueCommand(command: encodeJSON(["op": "set", "songs": [["id": "a", "title": "A"], ["id": "b", "title": "B"], ["id": "c", "title": "C"]]]))
        music.page = "Albums"; music.kind = "albums"; music.rows = [["id": "album", "name": "Album"]]
        music.queueAction("remove", row: ["id": "b", "queueLane": "main", "queueIndex": 1])
        XCTAssertEqual(try queueIds(core), ["a", "c"])
        XCTAssertEqual(music.page, "Albums")
        XCTAssertEqual(music.rows.first?["id"] as? String, "album")
    }
    func testTrackTapsKeepShuffleExceptAlbums() throws {
        let (music, core) = try makeMusic()
        let songs: [[String: Any]] = (1...4).map { ["id": "s\($0)", "title": "S\($0)"] }
        _ = try core.queueCommand(command: encodeJSON(["op": "set", "songs": songs]))
        _ = music.command(["op": "shuffle"])
        XCTAssertTrue(music.shuffleOn)
        music.page = "Home"; music.kind = "songs"; music.rows = songs
        music.select(songs[2], at: 2)
        XCTAssertTrue(music.shuffleOn, "Home and search taps keep the listener's shuffle")
        XCTAssertEqual(music.currentSong["id"] as? String, "s3")
        music.page = "Collection"; music.collectionContext = ["kind": "albums", "id": "fake-album", "name": "Fake album"]
        music.select(songs[0], at: 0)
        XCTAssertFalse(music.shuffleOn, "Albums play in order")
        XCTAssertEqual(try queueIds(core), ["s1", "s2", "s3", "s4"])
        XCTAssertEqual(music.currentSong["id"] as? String, "s1")
        music.pause()
    }
    func testSearchChipSurvivesCollectionAndBack() throws {
        let (music, _) = try makeMusic()
        music.page = "Omnibar"; music.searchQuery = "fake"
        music.applyCatalog(["kind": "songs", "rows": [["id": "s1", "title": "One"], ["id": "al", "title": "Album", "browseKind": "albums"]],
                            "extra": ["sections": [["title": "In your library", "start": 0, "count": 1], ["title": "Albums", "start": 1, "count": 1]]]])
        music.setSearchFilter("albums")
        XCTAssertEqual(music.rows.compactMap { $0["id"] as? String }, ["al"])
        music.select(music.rows[0], at: 0)
        XCTAssertEqual(music.page, "Collection")
        music.back()
        XCTAssertEqual(music.searchFilter, "albums")
        XCTAssertEqual(music.rows.compactMap { $0["id"] as? String }, ["al"])
        music.setSearchFilter("all")
        XCTAssertEqual(music.rows.count, 2, "The full results come back when the chip is cleared")
    }
    func testLikesRefreshTheCurrentSongInPlace() throws {
        let (music, core) = try makeMusic()
        _ = try core.queueCommand(command: encodeJSON(["op": "set", "songs": [["id": "a", "title": "A"]]]))
        music.currentSong = ["id": "a", "title": "A"]
        music.refreshCurrentMetadata("a", patch: ["starred": "saved", "title": "Renamed"])
        XCTAssertTrue(present(music.currentSong["starred"]))
        XCTAssertEqual(music.current, "Renamed")
        XCTAssertNil(music.player.currentItem, "Metadata refreshes never rebuild audio")
        music.refreshCurrentMetadata("a", patch: ["starred": nil])
        XCTAssertFalse(present(music.currentSong["starred"]))
    }
    func testInterruptionsResumeOnlyWhenPlayingAndAllowed() throws {
        let (music, _) = try makeMusic()
        music.interruption(began: true, shouldResume: false)
        XCTAssertFalse(music.resumeAfterInterruption, "An idle player ignores stale interruptions")
        music.playing = true; music.wantsPlayback = true
        music.interruption(began: true, shouldResume: false)
        XCTAssertFalse(music.playing)
        XCTAssertTrue(music.resumeAfterInterruption)
        music.interruption(began: false, shouldResume: false)
        XCTAssertFalse(music.wantsPlayback, "Without shouldResume the user restarts playback")
        XCTAssertFalse(music.resumeAfterInterruption)
    }
    func testMediaServicesResetRecreatesThePlayerAndRemainsPaused() throws {
        let (music, _) = try makeMusic()
        let previousPlayer = music.player
        music.mediaServicesReset()
        XCTAssertFalse(music.player === previousPlayer)
        XCTAssertFalse(music.playing)
        XCTAssertFalse(music.wantsPlayback)
        XCTAssertNotNil(music.timeObserver)
    }
}
