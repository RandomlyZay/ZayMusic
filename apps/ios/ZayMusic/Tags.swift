import SwiftUI

struct TagSelection: Identifiable {
    let id = UUID()
    let song: [String: Any]
}

/// Stock form over the core's `editTags`, with metadata candidates from `metadataLookup`.
struct TagEditorSheet: View {
    @EnvironmentObject var music: Music
    @Environment(\.dismiss) private var dismiss
    let song: [String: Any]
    @State private var title = ""
    @State private var artist = ""
    @State private var album = ""
    @State private var track = ""
    @State private var year = ""
    @State private var genre = ""
    @State private var lookup = ""
    @State private var candidates: [[String: Any]] = []
    @State private var searching = false
    @State private var lookupMessage = ""
    @State private var saving = false
    @State private var loaded = false

    var body: some View {
        NavigationStack {
            Form {
                Section("Song") {
                    TextField("Title", text: $title).accessibilityIdentifier("tag-title")
                    TextField("Artist", text: $artist).accessibilityIdentifier("tag-artist")
                    TextField("Album", text: $album).accessibilityIdentifier("tag-album")
                    TextField("Genre", text: $genre)
                }
                Section {
                    TextField("Track number", text: $track).keyboardType(.numberPad)
                    TextField("Year", text: $year).keyboardType(.numberPad)
                }
                Section {
                    HStack {
                        TextField("Artist and title", text: $lookup)
                            .autocorrectionDisabled()
                            .submitLabel(.search)
                            .onSubmit { Task { await find() } }
                        if searching { ProgressView() } else {
                            Button("Find") { Task { await find() } }
                                .disabled(lookup.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                        }
                    }
                    if !lookupMessage.isEmpty { Text(lookupMessage).font(.footnote).foregroundStyle(.secondary) }
                    ForEach(Array(candidates.enumerated()), id: \.offset) { _, candidate in
                        Button { apply(candidate) } label: { candidateRow(candidate) }
                            .buttonStyle(.plain)
                    }
                } header: { Text("Find metadata") } footer: { Text("Choose a match to fill the form, then review and save.") }
                if !music.error.isEmpty {
                    Section { Label(music.error, systemImage: "exclamationmark.triangle.fill").foregroundStyle(.orange) }
                }
            }
            .navigationTitle("Edit tags")
            .navigationBarTitleDisplayMode(.inline)
            .scrollDismissesKeyboard(.interactively)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    if saving { ProgressView() } else {
                        Button("Save") { Task { await save() } }
                            .disabled(title.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                            .accessibilityIdentifier("tag-save")
                    }
                }
            }
            .onAppear {
                guard !loaded else { return }
                loaded = true
                title = song["title"] as? String ?? ""
                artist = song["artist"] as? String ?? ""
                album = song["album"] as? String ?? ""
                genre = song["genre"] as? String ?? ""
                track = (song["track"] as? NSNumber).map { "\($0.intValue)" } ?? ""
                year = (song["year"] as? NSNumber).map { "\($0.intValue)" } ?? ""
                lookup = [artist, title].filter { !$0.isEmpty }.joined(separator: " ")
                music.error = ""
            }
        }
        .presentationDetents([.large])
    }

    func candidateRow(_ candidate: [String: Any]) -> some View {
        let item: [String: Any] = ["title": candidate["title"] ?? "", "artworkUrl": candidate["artwork"] ?? candidate["artworkUrl"] ?? ""]
        let details = [candidate["artist"] as? String, candidate["album"] as? String, (candidate["year"] as? NSNumber).map { "\($0.intValue)" }]
            .compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: " · ")
        return HStack(spacing: 12) {
            ArtworkView(item: item, kind: .album, size: 44)
            VStack(alignment: .leading, spacing: 2) {
                Text(candidate["title"] as? String ?? "Untitled").lineLimit(1)
                Text(details).font(.caption).foregroundStyle(.secondary).lineLimit(1)
            }
            Spacer(minLength: 8)
            if let source = candidate["source"] as? String, !source.isEmpty {
                Text(source.capitalized).font(.caption2).foregroundStyle(.secondary)
            }
        }
        .contentShape(Rectangle())
    }

    func find() async {
        let query = lookup.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let core = music.core, !query.isEmpty else { return }
        searching = true; lookupMessage = ""
        defer { searching = false }
        do {
            let text = try await Task.detached { try core.metadataLookup(query: query) }.value
            let value = (try? JSONSerialization.jsonObject(with: Data(text.utf8))) ?? [:]
            candidates = (value as? [[String: Any]]) ?? ((value as? [String: Any])?["candidates"] as? [[String: Any]] ?? [])
            if candidates.isEmpty { lookupMessage = "No matches. Try a shorter search." }
        } catch { candidates = []; lookupMessage = "Metadata search is unavailable. Check the connection and retry." }
    }

    func apply(_ candidate: [String: Any]) {
        for (key, field) in [("title", $title), ("artist", $artist), ("album", $album), ("genre", $genre)] {
            if let value = candidate[key] as? String, !value.isEmpty { field.wrappedValue = value }
        }
        if let value = candidate["year"] as? NSNumber, value.intValue > 0 { year = "\(value.intValue)" }
        if let value = candidate["track"] as? NSNumber, value.intValue > 0 { track = "\(value.intValue)" }
    }

    func save() async {
        saving = true
        defer { saving = false }
        let edits = tagEdits(title: title, artist: artist, album: album, track: track, year: year, genre: genre)
        if await music.library("editTags", item: song, kind: "songs", extra: ["edits": edits]) { dismiss() }
    }
}
