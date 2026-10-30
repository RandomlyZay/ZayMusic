import Foundation

struct Track: Codable, Identifiable, Hashable {
    var id: String
    var title: String
    var artist: String = ""
    var album: String = ""
    var duration: Double = 0
}
func decodeJSON(_ text: String) -> [String: Any] { (try? JSONSerialization.jsonObject(with: Data(text.utf8))) as? [String: Any] ?? [:] }
func decodeRows(_ text: String) -> [[String: Any]] { (try? JSONSerialization.jsonObject(with: Data(text.utf8))) as? [[String: Any]] ?? [] }
/// JSONSerialization raises an Objective-C exception for non-JSON values; check before encoding.
func encodeJSON(_ value: Any) -> String {
    guard JSONSerialization.isValidJSONObject(value), let data = try? JSONSerialization.data(withJSONObject: value) else { return "{}" }
    return String(decoding: data, as: UTF8.self)
}

let defaultServerAddress = "https://fedora-server.tailfc5dc.ts.net"

/// A server address that is safe to persist: HTTP(S) with a host and no credentials,
/// query or fragment. Mirrors the shared core's `set_preference` validation.
func validatedServerAddress(_ text: String) -> String? {
    let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
    guard let parts = URLComponents(string: trimmed), let scheme = parts.scheme?.lowercased(),
          ["http", "https"].contains(scheme), let host = parts.host, !host.isEmpty,
          parts.user == nil, parts.password == nil, parts.query == nil, parts.fragment == nil else { return nil }
    return trimmed
}

/// JSON `null` arrives as NSNull; treat it, and empty strings, as an absent value.
func present(_ value: Any?) -> Bool {
    guard let value, !(value is NSNull) else { return false }
    if let text = value as? String { return !text.isEmpty }
    return true
}

/// Mixed search rows carry a non-song `browseKind`; only song rows may enter the queue.
func isSongRow(_ row: [String: Any]) -> Bool { (row["browseKind"] as? String).map { $0 == "songs" } ?? true }
func playableRows(_ rows: [[String: Any]]) -> [[String: Any]] { rows.filter(isSongRow) }
/// Server-library songs. YouTube previews must be added to the library before saving or downloading.
func isLibrarySong(_ row: [String: Any]) -> Bool {
    guard isSongRow(row), let id = row["id"] as? String, !id.isEmpty else { return false }
    return !id.hasPrefix("yt:")
}
func downloadableRows(_ rows: [[String: Any]]) -> [[String: Any]] { rows.filter(isLibrarySong) }
/// Rows that play together when one is chosen: the tapped row's section (Home shelves,
/// search groups) or the whole list. Returns the songs and the tapped song's position.
func playbackContext(_ rows: [[String: Any]], sections: [Range<Int>], selected: Int) -> (songs: [[String: Any]], index: Int)? {
    guard rows.indices.contains(selected), isSongRow(rows[selected]) else { return nil }
    let range = sections.first { $0.contains(selected) && $0.upperBound <= rows.count } ?? rows.indices
    let songs = playableRows(Array(rows[range]))
    let index = playableRows(Array(rows[range.lowerBound..<selected])).count
    return (songs, index)
}
/// The collection a tapped row plays from. Home shelves and search results have none, so the
/// core keeps the listener's shuffle; albums always play in order.
func playContextValue(page: String, collection: [String: Any]) -> [String: Any] {
    if page == "Liked Songs" { return ["kind": "liked", "id": "liked", "name": "Liked Songs"] }
    return page == "Collection" ? collection : [:]
}

/// Search chips narrow the core's ranked mixed sections, keeping its order and section titles.
func filterSearch(_ rows: [[String: Any]], sections: [[String: Any]], filter: String) -> (rows: [[String: Any]], sections: [[String: Any]]) {
    guard filter != "all" else { return (rows, sections) }
    func keep(_ row: [String: Any]) -> Bool {
        let kind = row["browseKind"] as? String ?? "songs"
        let preview = (row["id"] as? String)?.hasPrefix("yt:") == true
        switch filter {
        case "songs": return kind == "songs"
        case "albums": return kind == "albums" || kind == "newAlbums"
        case "library": return kind != "newAlbums" && !preview
        default: return true
        }
    }
    var ranges: [(title: String, range: Range<Int>)] = sections.compactMap { section in
        guard let start = section["start"] as? Int, let count = section["count"] as? Int,
              start >= 0, count >= 0, start + count <= rows.count else { return nil }
        return (section["title"] as? String ?? "", start..<(start + count))
    }
    if ranges.isEmpty { ranges = [("", rows.indices)] }
    var kept: [[String: Any]] = []
    var keptSections: [[String: Any]] = []
    for (title, range) in ranges {
        let matches = rows[range].filter(keep)
        guard !matches.isEmpty else { continue }
        if !title.isEmpty { keptSections.append(["title": title, "start": kept.count, "count": matches.count]) }
        kept += matches
    }
    return (kept, keptSections)
}

/// A "Did you mean" query from the core's search extras: text or an artist/title pair.
func searchCorrection(_ value: Any?) -> String {
    if let text = value as? String { return text.trimmingCharacters(in: .whitespacesAndNewlines) }
    guard let pair = value as? [String: Any] else { return "" }
    return [pair["artist"] as? String, pair["title"] as? String].compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: " ")
}

/// ZayFetch job states grouped as the Jobs filters show them.
func jobBucket(_ status: String) -> String {
    switch status {
    case "queued", "pending", "running", "downloading": return "active"
    case "failed", "cancelled", "error": return "attention"
    default: return "done"
    }
}

/// Settings copy for a YouTube Music connection that blocks imports and new downloads.
func youtubeAttention(_ youtube: [String: Any]) -> String {
    guard !youtube.isEmpty else { return "" }
    if youtube["configured"] as? Bool != true { return "Connect YouTube Music in Settings to import playlists and add new songs." }
    if youtube["verified"] as? Bool != true { return "YouTube Music needs attention. Reconnect it in Settings." }
    return ""
}

/// Offline means the server could not be reached, not that the server rejected the stream.
func isOfflineError(_ error: Error?) -> Bool {
    let offline = [NSURLErrorNotConnectedToInternet, NSURLErrorNetworkConnectionLost, NSURLErrorCannotFindHost,
                   NSURLErrorCannotConnectToHost, NSURLErrorTimedOut, NSURLErrorDNSLookupFailed,
                   NSURLErrorInternationalRoamingOff, NSURLErrorDataNotAllowed]
    var next = error.map { $0 as NSError }
    while let current = next {
        if current.domain == NSURLErrorDomain && offline.contains(current.code) { return true }
        next = current.userInfo[NSUnderlyingErrorKey] as? NSError
    }
    return false
}
/// What the player says when an item cannot play. Never includes URLs or raw errors.
func playbackFailureMessage(_ error: Error?, preview: Bool, local: Bool) -> String {
    if local { return "The downloaded file can't be played. Remove the download and download it again." }
    if isOfflineError(error) {
        return preview ? "Previews need a connection to your server." : "This song isn't downloaded. Connect to your server, or download it to play offline."
    }
    return preview ? "This preview couldn't be played. Try again, or add the song to your library." : "This song couldn't be played. Check the connection and try again."
}

/// The library album or artist a song belongs to, as a row `openCollection` accepts. Previews have none.
func albumLink(_ song: [String: Any]) -> [String: Any]? {
    guard isLibrarySong(song), let id = song["albumId"] as? String, !id.isEmpty else { return nil }
    var row: [String: Any] = ["id": id, "name": song["album"] as? String ?? "Album"]
    for key in ["coverArt", "artist"] where present(song[key]) { row[key] = song[key] }
    return row
}
func artistLink(_ song: [String: Any]) -> [String: Any]? {
    guard isLibrarySong(song), let id = song["artistId"] as? String, !id.isEmpty else { return nil }
    return ["id": id, "name": song["artist"] as? String ?? "Artist"]
}

/// Tag edits for the core's `editTags`: blank numbers clear the field, text is trimmed.
func tagEdits(title: String, artist: String, album: String, track: String, year: String, genre: String) -> [String: Any] {
    func text(_ value: String) -> String { value.trimmingCharacters(in: .whitespacesAndNewlines) }
    func number(_ value: String) -> Any { Int(text(value)).flatMap { $0 > 0 ? $0 : nil } ?? NSNull() }
    return ["title": text(title), "artist": text(artist), "album": text(album), "track": number(track), "year": number(year), "genre": text(genre)]
}
