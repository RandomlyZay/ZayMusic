import SwiftUI

/// Spotify-style play queue over the shared core's lanes. It edits the engine directly and
/// never loads into the shared catalog, so the tab underneath keeps its page and navigation.
struct NativeQueueSheet: View {
    @EnvironmentObject var music: Music
    @Environment(\.dismiss) private var dismiss
    @State private var rows: [[String: Any]] = []

    var body: some View {
        NavigationStack {
            List {
                Section("Now playing") {
                    HStack(spacing: 12) {
                        ArtworkView(item: music.currentSong, kind: .song, size: 48)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(music.current).font(.headline).foregroundStyle(music.currentSong.isEmpty ? Color.primary : Theme.accent).lineLimit(1)
                            if let artist = music.currentSong["artist"] as? String, !artist.isEmpty {
                                Text(artist).font(.subheadline).foregroundStyle(.secondary).lineLimit(1)
                            }
                        }
                    }
                    .accessibilityElement(children: .combine)
                    .accessibilityIdentifier("queue-now-playing")
                }
                lane("priority", title: "Next in queue")
                lane("main", title: "Main queue")
                if rows.isEmpty {
                    Text("Nothing queued").foregroundStyle(.secondary)
                }
                Section {
                    Toggle("Autoplay", isOn: Binding(get: { music.autoplay }, set: { music.setAutoplay($0) })).tint(Theme.accent)
                } footer: {
                    if music.autoplay && !music.radioError.isEmpty {
                        Label(music.radioError, systemImage: "exclamationmark.triangle").foregroundStyle(.orange)
                            .accessibilityIdentifier("queue-radio-error")
                    } else {
                        Text("When the queue ends, keep playing similar songs.")
                    }
                }
            }
            .accessibilityIdentifier("queue-sheet")
            .navigationTitle("Play queue")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } }
                ToolbarItemGroup(placement: .bottomBar) {
                    Button { _ = music.command(["op": "shuffle"]); reload() } label: { Image(systemName: "shuffle") }
                        .foregroundStyle(music.shuffleOn ? Theme.accent : Color.primary)
                        .accessibilityLabel(music.shuffleOn ? "Shuffle on" : "Shuffle off")
                    Spacer()
                    Button { _ = music.command(["op": "repeat"]); reload() } label: {
                        Image(systemName: music.repeatMode == "one" ? "repeat.1" : "repeat")
                    }
                    .foregroundStyle(music.repeatMode == "off" ? Color.primary : Theme.accent)
                    .accessibilityLabel("Repeat \(music.repeatMode)")
                }
            }
            // Radio top-ups arrive without a notification, so a slow refresh keeps the lanes current.
            .task {
                reload()
                while !Task.isCancelled {
                    do { try await Task.sleep(nanoseconds: 2_000_000_000) } catch { break }
                    reload()
                }
            }
            .onChange(of: music.current) { reload() }
        }
    }

    @ViewBuilder func lane(_ name: String, title: String) -> some View {
        let entries = rows.filter { $0["queueLane"] as? String == name }
        if !entries.isEmpty {
            Section(title) {
                ForEach(Array(entries.enumerated()), id: \.offset) { position, row in
                    entry(row, first: position == 0, last: position == entries.count - 1)
                }
            }
        }
    }

    func entry(_ row: [String: Any], first: Bool, last: Bool) -> some View {
        let id = row["id"] as? String ?? "unknown"
        return HStack(spacing: 12) {
            Button { jump(row) } label: {
                HStack(spacing: 12) {
                    ArtworkView(item: row, kind: .song, size: 40)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(rowTitle(row)).lineLimit(1)
                        Text(row["artist"] as? String ?? "").font(.caption).foregroundStyle(.secondary).lineLimit(1)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("queue-row-\(id)")
            Menu {
                Button { edit("move", row, -1) } label: { Label("Move up", systemImage: "arrow.up") }.disabled(first)
                Button { edit("move", row, 1) } label: { Label("Move down", systemImage: "arrow.down") }.disabled(last)
                Button(role: .destructive) { edit("remove", row, 0) } label: { Label("Remove from queue", systemImage: "minus.circle") }
            } label: {
                Image(systemName: "ellipsis").frame(width: 44, height: 44).contentShape(Rectangle())
            }
            .accessibilityLabel("Queue actions for \(rowTitle(row))")
            .accessibilityIdentifier("queue-actions-\(id)")
        }
        .swipeActions(edge: .trailing) {
            Button(role: .destructive) { edit("remove", row, 0) } label: { Label("Remove from queue", systemImage: "minus.circle") }
        }
    }

    func reload() {
        guard let core = music.core else { rows = []; return }
        rows = decodeJSON((try? core.browse(page: "Queue", query: "")) ?? "{}")["rows"] as? [[String: Any]] ?? []
        music.applyModes(decodeJSON((try? core.queueCommand(command: "{}")) ?? "{}"))
    }
    func edit(_ op: String, _ row: [String: Any], _ delta: Int) { music.queueAction(op, row: row, delta: delta); reload() }
    /// Queue rows are songs whatever page the tab underneath shows.
    func jump(_ row: [String: Any]) { var song = row; song["browseKind"] = "songs"; music.select(song); reload() }
}
