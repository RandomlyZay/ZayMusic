import Foundation

@main
struct NativeSwiftCheck {
    static func main() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let core = try NativeCore(server: "https://example.invalid", directory: root.path)
        let initial = decodeJSON(try core.queueCommand(command: encodeJSON([
            "op": "set", "songs": [["id": "fake-a", "title": "First"], ["id": "fake-b", "title": "Second"]]
        ])))
        precondition((initial["current"] as? [String: Any])?["id"] as? String == "fake-a")
        let next = decodeJSON(try core.queueCommand(command: "{\"op\":\"next\"}"))
        precondition((next["current"] as? [String: Any])?["id"] as? String == "fake-b")
        let stopped = decodeJSON(try core.queueCommand(command: "{\"op\":\"next\"}"))
        precondition(stopped["continue"] as? Bool == false)
        try core.setAutoplay(enabled: false)
        let radio = try core.ensureRadio()
        precondition(radio == false)
        let shuffle = decodeJSON(try core.startShuffle(songsJson: encodeJSON([["id": "fake-a", "title": "First"], ["id": "fake-b", "title": "Second"]]), density: "off"))
        let snapshot = decodeJSON(try core.queueCommand(command: "{}"))
        precondition(shuffle["applied"] as? Bool == true)
        precondition(shuffle["generation"] as? UInt64 == snapshot["generation"] as? UInt64)
        precondition(snapshot["autoplay"] as? Bool == false)
        let metadata = encodeJSON(["id": "offline", "title": "Offline", "suffix": "wav"])
        let plan = decodeJSON(try core.queueDownload(songJson: metadata))
        let ticket = plan["ticket"] as! UInt64
        let began = try core.beginDownload(id: "offline", ticket: ticket, taskKey: "ios:fixture")
        precondition(began)
        let file = root.appendingPathComponent("temporary.wav")
        try Data([0, 1, 2, 3]).write(to: file)
        try core.completeDownload(id: "offline", ticket: ticket, temporaryFile: file.path)
        let offlineURL = try core.mediaUrl(songJson: metadata, download: false)
        precondition(offlineURL.hasPrefix("file:"))
        let stats = decodeJSON(try core.storageStats())
        precondition(stats["readyBytes"] as? UInt64 == 4)
        try core.deleteDownload(id: "offline")
        let playlists = try NativeCore(server: ProcessInfo.processInfo.environment["NATIVE_FIXTURE_URL"]!, directory: root.appendingPathComponent("playlists").path)
        func edit(_ command: [String: Any]) throws -> [String: Any] { decodeJSON(try playlists.playlistCommand(command: encodeJSON(command))) }
        let created = try edit(["op": "create", "name": "Swift fixture", "songs": [["id": "fixture-1"], ["id": "fixture-2"], ["id": "fixture-3"]]])
        let id = (created["playlist"] as! [String: Any])["id"] as! String
        _ = try edit(["op": "move", "id": id, "from": 2, "to": 0, "songId": "fixture-3"])
        _ = try edit(["op": "rename", "id": id, "name": "Renamed fixture"])
        let collection = decodeJSON(try playlists.openCollection(kind: "playlists", id: id))
        let rows = collection["rows"] as! [[String: Any]]
        precondition(rows.map { $0["id"] as! String } == ["fixture-3", "fixture-1", "fixture-2"])
        precondition(rows[0]["playlistIndex"] as? Int == 0)
        precondition(collection["title"] as? String == "Renamed fixture")
        _ = try edit(["op": "remove", "id": id, "index": 1, "songId": "fixture-1"])
        _ = try edit(["op": "delete", "id": id])
        let listing = decodeJSON(try playlists.browse(page: "Playlists", query: ""))["rows"] as! [[String: Any]]
        precondition(!listing.contains { $0["id"] as? String == id })
        let cover = try playlists.artwork(itemJson: encodeJSON(["id": "fixture-1", "coverArt": "al-fixture-1"]), size: 96)
        precondition(cover.starts(with: [0x89, 0x50, 0x4E, 0x47]), "Artwork must arrive as encoded image bytes")
        do {
            _ = try playlists.artwork(itemJson: encodeJSON(["id": "no-art", "title": "No art"]), size: 96)
            preconditionFailure("Rows without art must fall back to the placeholder")
        } catch {}
        let historySnapshot = decodeJSON(try playlists.queueCommand(command: encodeJSON(["op": "set", "songs": [["id": "fixture-1", "title": "Fixture song 1", "duration": 60]]])))
        let epoch = (historySnapshot["playbackEpoch"] as! NSNumber).uint64Value
        try playlists.playbackProgress(epoch: epoch, songId: "fixture-1", position: 0, duration: 60, playing: true)
        var recent: [[String: Any]] = []
        for _ in 0..<100 {
            recent = try JSONSerialization.jsonObject(with: Data(playlists.recentSongs().utf8)) as! [[String: Any]]
            if !recent.isEmpty { break }
            Thread.sleep(forTimeInterval: 0.01)
        }
        precondition(recent.first?["id"] as? String == "fixture-1", "Listening history must cross the Swift boundary")
        print("Swift → UniFFI → Rust queue, playback settings, offline catalog, playlist HTTP, artwork and listening history integration passed")
    }
}
