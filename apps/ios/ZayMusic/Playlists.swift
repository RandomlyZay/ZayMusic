import SwiftUI

extension Music {
    /// Returns the core's result, or nil when the edit failed.
    @discardableResult
    func mutatePlaylist(_ command: [String: Any], refresh: Bool) async -> [String: Any]? {
        guard let core else { return nil }
        let generation = loadGeneration
        do {
            let value = decodeJSON(try await Task.detached { try core.playlistCommand(command: encodeJSON(command)) }.value)
            guard generation == loadGeneration else { return value }
            if refresh {
                if page == "Collection", command["op"] as? String != "delete", let id = collectionContext["id"] as? String {
                    let text = try await Task.detached { try core.openCollection(kind: "playlists", id: id) }.value
                    guard generation == loadGeneration else { return value }
                    applyCatalog(decodeJSON(text))
                } else { await load("Playlists") }
            }
            notice = value["warning"] as? String ?? ((value["queued"] as? Int ?? 0) > 0 ? "Queued for download and playlist." : "Playlist updated.")
            return value
        } catch { if generation == loadGeneration { self.error = "Playlist could not be updated. Refresh and try again." }; return nil }
    }
    func editPlaylistTrack(_ operation: String, row: [String: Any], delta: Int = 0) async {
        let index = row["playlistIndex"] as? Int ?? 0
        var command: [String: Any] = ["op": operation, "id": row["playlistId"] ?? "", "songId": row["id"] ?? "", "index": index]
        if operation == "move" { command["from"] = index; command["to"] = max(0, min(rows.count - 1, index + delta)) }
        await mutatePlaylist(command, refresh: true)
    }
}

/// What the picker adds: one song, a bulk selection, or an album saved through the core.
struct PlaylistSelection: Identifiable {
    let id = UUID()
    let songs: [[String: Any]]
    var album: [String: Any]? = nil
    init(song: [String: Any]) { songs = [song] }
    init(songs: [[String: Any]]) { self.songs = songs }
    init(album: [String: Any]) { songs = []; self.album = album }
}

struct PlaylistPickerSheet: View {
    @EnvironmentObject var music: Music
    @Environment(\.dismiss) var dismiss
    let selection: PlaylistSelection
    @State private var choices: [[String: Any]] = []
    @State private var name = ""
    @State private var busy = false
    private var song: [String: Any] { selection.album ?? selection.songs.first ?? [:] }
    private var heading: String {
        if let album = selection.album { return rowTitle(album) }
        return selection.songs.count == 1 ? song["title"] as? String ?? "Selected song" : "\(selection.songs.count) songs"
    }
    var body: some View {
        NavigationStack {
            List {
                HStack(spacing: 12) {
                    ArtworkView(item: song, kind: selection.album == nil ? .song : .album, size: 48)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(heading)
                            .font(.body.weight(.semibold))
                            .accessibilityIdentifier("playlist-selected-song")
                        if let artist = song["artist"] as? String, !artist.isEmpty {
                            Text(artist).font(.subheadline).foregroundStyle(.secondary)
                        }
                    }
                }
                if selection.album != nil {
                    Text("The album is added to your library, then its songs join the playlist as they finish downloading.")
                        .font(.footnote).foregroundStyle(.secondary)
                }
                if !music.error.isEmpty { Text(music.error).foregroundStyle(.orange) }
                Section("Create playlist") {
                    TextField("New playlist name", text: $name)
                    Button("Create and add") { Task { await create() } }
                        .disabled(busy || name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }
                Section("Your playlists") {
                    ForEach(Array(choices.enumerated()), id: \.offset) { _, playlist in
                        let name = playlist["name"] as? String ?? "Playlist"
                        Button {
                            Task { await add(to: playlist["id"] as? String ?? "") }
                        } label: {
                            HStack(spacing: 12) {
                                ArtworkView(item: playlist, kind: .playlist, size: 44)
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(name).foregroundStyle(.primary)
                                    if let count = playlist["songCount"] as? Int {
                                        Text("\(count) song\(count == 1 ? "" : "s")").font(.subheadline).foregroundStyle(.secondary)
                                    }
                                }
                            }
                        }
                        .disabled(busy)
                        // Keep the control's name exactly the playlist name; the count is its value.
                        .accessibilityLabel(name)
                        .accessibilityValue((playlist["songCount"] as? Int).map { "\($0) songs" } ?? "")
                    }
                }
            }.navigationTitle("Add to playlist")
                .toolbar { Button("Cancel") { dismiss() } }
                .task {
                    guard let core = music.core else { return }
                    do {
                        let text = try await Task.detached { try core.browse(page: "Playlists", query: "") }.value
                        choices = decodeJSON(text)["rows"] as? [[String: Any]] ?? []
                    } catch { music.error = "Could not load playlists." }
                }
        }.presentationDetents([.medium, .large])
    }
    func create() async {
        busy = true; defer { busy = false }
        guard let album = selection.album else {
            if await music.mutatePlaylist(["op": "create", "name": name, "songs": selection.songs], refresh: false) != nil { dismiss() }
            return
        }
        // Albums are acquired by the core; create the empty target first, then save into it.
        guard let created = await music.mutatePlaylist(["op": "create", "name": name, "songs": [] as [Any]], refresh: false),
              let id = (created["playlist"] as? [String: Any])?["id"] as? String else { return }
        if await music.library("acquireAlbum", item: album, kind: "newAlbums", extra: ["playlistId": id]) { dismiss() }
    }
    func add(to id: String) async {
        busy = true; defer { busy = false }
        if let album = selection.album {
            if await music.library("acquireAlbum", item: album, kind: "newAlbums", extra: ["playlistId": id]) { dismiss() }
        } else if await music.mutatePlaylist(["op": "add", "id": id, "songs": selection.songs], refresh: false) != nil {
            dismiss()
        }
    }
}
