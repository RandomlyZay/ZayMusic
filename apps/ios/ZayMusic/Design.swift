import SwiftUI
import AVKit
import UIKit

/// Visual tokens. Green is reserved for play, now-playing and "on" states.
enum Theme {
    static let accent = Color(red: 0.118, green: 0.843, blue: 0.376)
    static let elevated = Color(white: 0.16)
    /// Muted gradient pairs for placeholder art; the Linux client uses the same hues.
    static let placeholders: [(Color, Color)] = [
        (Color(red: 0.29, green: 0.23, blue: 0.56), Color(red: 0.12, green: 0.11, blue: 0.23)),
        (Color(red: 0.56, green: 0.23, blue: 0.36), Color(red: 0.18, green: 0.08, blue: 0.14)),
        (Color(red: 0.18, green: 0.44, blue: 0.56), Color(red: 0.06, green: 0.15, blue: 0.20)),
        (Color(red: 0.56, green: 0.42, blue: 0.18), Color(red: 0.20, green: 0.14, blue: 0.06)),
        (Color(red: 0.18, green: 0.56, blue: 0.39), Color(red: 0.06, green: 0.20, blue: 0.13)),
        (Color(red: 0.44, green: 0.18, blue: 0.56), Color(red: 0.15, green: 0.06, blue: 0.20)),
        (Color(red: 0.56, green: 0.25, blue: 0.18), Color(red: 0.20, green: 0.08, blue: 0.06)),
        (Color(red: 0.25, green: 0.31, blue: 0.44), Color(red: 0.08, green: 0.11, blue: 0.15)),
    ]
    static func placeholder(for name: String) -> (Color, Color) {
        let hash = name.utf8.reduce(UInt32(7)) { $0 &* 31 &+ UInt32($1) }
        return placeholders[Int(hash % UInt32(placeholders.count))]
    }
}

enum ArtKind {
    case song, album, artist, playlist, genre, liked, other
    init(catalogKind: String) {
        switch catalogKind {
        case "songs": self = .song
        case "albums", "newAlbums": self = .album
        case "artists": self = .artist
        case "playlists": self = .playlist
        case "genres": self = .genre
        case "liked": self = .liked
        default: self = .other
        }
    }
    var symbol: String {
        switch self {
        case .song: return "music.note"
        case .album: return "opticaldisc"
        case .artist: return "person.fill"
        case .playlist: return "music.note.list"
        case .genre: return "guitars"
        case .liked: return "heart.fill"
        case .other: return "square.stack"
        }
    }
}

func rowTitle(_ row: [String: Any]) -> String {
    row["title"] as? String ?? row["name"] as? String ?? row["value"] as? String ?? "Untitled"
}

func clockText(_ seconds: Double) -> String {
    let total = Int(max(0, seconds.isFinite ? seconds : 0))
    return total >= 3600 ? String(format: "%d:%02d:%02d", total / 3600, total / 60 % 60, total % 60) : String(format: "%d:%02d", total / 60, total % 60)
}

/// Cover art from the shared core's gateway cache, over a deliberate gradient placeholder.
struct ArtworkView: View {
    @EnvironmentObject var music: Music
    let item: [String: Any]
    var kind: ArtKind = .song
    var size: CGFloat = 48
    var symbol: String? = nil
    @State private var image: UIImage?

    private var key: String {
        [item["artworkUrl"], item["coverArt"], item["albumId"], item["id"]].compactMap { $0 as? String }.joined(separator: "|")
    }
    private var pixels: Int { size <= 56 ? 120 : size <= 120 ? 240 : 600 }

    var body: some View {
        let colors = kind == .liked
            ? (Color(red: 0.27, green: 0.04, blue: 0.96), Color(red: 0.77, green: 0.94, blue: 0.85))
            : Theme.placeholder(for: rowTitle(item))
        ZStack {
            LinearGradient(colors: [colors.0, colors.1], startPoint: .topLeading, endPoint: .bottomTrailing)
            Image(systemName: symbol ?? kind.symbol)
                .font(.system(size: size * 0.38, weight: .semibold))
                .foregroundStyle(.white.opacity(kind == .liked ? 1 : 0.75))
            if let image {
                Image(uiImage: image).resizable().scaledToFill()
            }
        }
        .frame(width: size, height: size)
        .clipShape(kind == .artist ? AnyShape(Circle()) : AnyShape(RoundedRectangle(cornerRadius: size >= 120 ? 8 : 4, style: .continuous)))
        .accessibilityHidden(true)
        .task(id: key) {
            image = nil
            guard kind != .liked, !key.isEmpty else { return }
            image = await music.artwork(item, size: pixels)
        }
    }
}

/// The system AirPlay / output route picker.
struct AirPlayButton: UIViewRepresentable {
    func makeUIView(context: Context) -> AVRoutePickerView {
        let view = AVRoutePickerView()
        view.tintColor = .white
        view.activeTintColor = UIColor(Theme.accent)
        return view
    }
    func updateUIView(_ uiView: AVRoutePickerView, context: Context) {}
}

/// Let the OS provide the Liquid Glass surface and positioning on iOS 26/27.
/// Earlier iOS releases retain a native material inset in each navigation stack.
struct SystemTabPlayer: ViewModifier {
    let open: () -> Void
    @ViewBuilder func body(content: Content) -> some View {
        if #available(iOS 26.0, *) {
            content.tabViewBottomAccessory {
                MiniPlayer(usesSystemBackground: true, open: open)
            }
        } else {
            content
        }
    }
}

struct LegacyPlayerInset: ViewModifier {
    let open: () -> Void
    @ViewBuilder func body(content: Content) -> some View {
        if #available(iOS 26.0, *) {
            content
        } else {
            content.safeAreaInset(edge: .bottom) { MiniPlayer(open: open) }
        }
    }
}

private struct NativeGlassButton: ViewModifier {
    @ViewBuilder func body(content: Content) -> some View {
        if #available(iOS 26.0, *) {
            content.buttonStyle(.glass).buttonBorderShape(.circle)
        } else {
            content.buttonStyle(.borderless)
        }
    }
}

/// Like / Unlike for library songs; previews offer Add to library instead (no likes for previews).
struct CurrentSongSaveButton: View {
    @EnvironmentObject var music: Music
    var size: Font = .title3
    var body: some View {
        let song = music.currentSongRow
        let id = song["id"] as? String ?? ""
        if id.hasPrefix("yt:") {
            Button { music.action("acquire", song) } label: {
                Image(systemName: "plus.circle").font(size).frame(width: 44, height: 44)
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Add to library")
            .accessibilityIdentifier("current-acquire")
        } else if !id.isEmpty {
            let liked = present(song["starred"])
            Button { Task { await music.library(liked ? "unsave" : "save", item: song, kind: "songs") } } label: {
                Image(systemName: liked ? "heart.fill" : "heart").font(size)
                    .foregroundStyle(liked ? Theme.accent : Color.primary)
                    .frame(width: 44, height: 44)
            }
            .buttonStyle(.plain)
            .accessibilityLabel(liked ? "Remove from Liked Songs" : "Add to Liked Songs")
            .accessibilityIdentifier("current-like")
        }
    }
}

/// Compact player docked above the tab bar inside each tab's safe area.
struct MiniPlayer: View {
    @EnvironmentObject var music: Music
    var usesSystemBackground = false
    let open: () -> Void

    @ViewBuilder var body: some View {
        if usesSystemBackground {
            controls.padding(.horizontal, 12)
        } else {
            controls
                .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                .padding(.horizontal, 8)
                .padding(.bottom, 6)
        }
    }

    private var controls: some View {
        HStack(spacing: 10) {
            Button(action: open) {
                HStack(spacing: 10) {
                    ArtworkView(item: music.currentSong, kind: .song, size: 40)
                    VStack(alignment: .leading, spacing: 1) {
                        Text(music.current).font(.subheadline.weight(.semibold)).lineLimit(1)
                        if !music.playbackError.isEmpty {
                            Text(music.playbackError).font(.caption).foregroundStyle(.orange).lineLimit(1)
                        } else if let artist = music.currentSong["artist"] as? String, !artist.isEmpty {
                            Text(artist).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                        }
                    }
                    Spacer(minLength: 0)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Now playing: \(music.current)")
            .accessibilityHint("Opens the player")
            .accessibilityIdentifier("mini-player")
            CurrentSongSaveButton()
            Button {
                music.playing ? music.pause() : music.resume()
            } label: {
                Image(systemName: music.playing ? "pause.fill" : "play.fill").font(.title3).frame(width: 44, height: 44)
            }
            .buttonStyle(.plain)
            .accessibilityLabel(music.playing ? "Pause" : "Play")
            Button { music.advance() } label: {
                Image(systemName: "forward.fill").font(.title3).frame(width: 44, height: 44)
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Next track")
        }
        .padding(.leading, 8)
        .padding(.vertical, 6)
        .overlay(alignment: .bottom) {
            TimelineView(.periodic(from: .now, by: 1)) { _ in
                ProgressView(value: music.progressFraction)
                    .tint(.white)
                    .scaleEffect(x: 1, y: 0.6, anchor: .bottom)
                    .padding(.horizontal, 10)
                    .accessibilityHidden(true)
            }
        }
    }
}

/// Full-screen player presented as a native sheet.
struct NowPlayingView: View {
    /// Opens an album or artist in the tab underneath; the parent closes this sheet.
    var goTo: (String, [String: Any]) -> Void = { _, _ in }
    @State private var showQueue = false
    @EnvironmentObject var music: Music
    @Environment(\.dismiss) var dismiss
    @State private var scrub: Double?
    @State private var playlistSelection: PlaylistSelection?
    @State private var tagEditing: TagSelection?

    var body: some View {
        let tint = Theme.placeholder(for: rowTitle(music.currentSong)).0
        GeometryReader { proxy in
            VStack(spacing: 16) {
                HStack {
                    Button { dismiss() } label: {
                        Image(systemName: "chevron.down").font(.title3.weight(.semibold)).frame(width: 44, height: 44)
                    }
                    .accessibilityLabel("Close player")
                    .modifier(NativeGlassButton())
                    Spacer()
                    Text("Now Playing").font(.subheadline.weight(.semibold))
                    Spacer()
                    songMenu
                }
                Spacer(minLength: 0)
                ArtworkView(item: music.currentSong, kind: .song, size: min(proxy.size.width - 48, proxy.size.height * 0.4, 360))
                    .shadow(color: .black.opacity(0.5), radius: 24, y: 12)
                Spacer(minLength: 0)
                HStack {
                    VStack(alignment: .leading, spacing: 4) {
                        Text(music.current).font(.title2.bold()).lineLimit(1)
                        Text(music.currentSong["artist"] as? String ?? " ").font(.body).foregroundStyle(.secondary).lineLimit(1)
                    }
                    Spacer()
                    CurrentSongSaveButton(size: .title2)
                }
                if !music.playbackError.isEmpty {
                    Label(music.playbackError, systemImage: "exclamationmark.triangle.fill")
                        .font(.footnote).foregroundStyle(.orange)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .accessibilityIdentifier("playback-error")
                } else if music.autoplay && !music.radioError.isEmpty {
                    Label(music.radioError, systemImage: "dot.radiowaves.left.and.right")
                        .font(.footnote).foregroundStyle(.secondary)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .accessibilityIdentifier("radio-error")
                }
                TimelineView(.periodic(from: .now, by: 0.5)) { _ in
                    let duration = max(music.duration, 1)
                    VStack(spacing: 4) {
                        Slider(value: Binding(get: { scrub ?? min(music.elapsed, duration) }, set: { scrub = $0 }), in: 0...duration, onEditingChanged: { editing in
                            if !editing, let target = scrub { music.seek(to: target); scrub = nil }
                        })
                        .tint(.white)
                        .accessibilityLabel("Playback position")
                        .accessibilityValue(clockText(scrub ?? music.elapsed))
                        HStack {
                            Text(clockText(scrub ?? music.elapsed))
                            Spacer()
                            Text("-" + clockText(max(0, music.duration - (scrub ?? music.elapsed))))
                        }
                        .font(.caption.monospacedDigit())
                        .foregroundStyle(.secondary)
                    }
                }
                HStack {
                    Button { _ = music.command(["op": "shuffle"]) } label: {
                        Image(systemName: "shuffle").font(.title3)
                    }
                    .foregroundStyle(music.shuffleOn ? Theme.accent : .white)
                    .accessibilityLabel(music.shuffleOn ? "Shuffle on" : "Shuffle off")
                    Spacer()
                    Button { music.previous() } label: { Image(systemName: "backward.fill").font(.title) }
                        .accessibilityLabel("Previous track")
                    Spacer()
                    Button { music.playing ? music.pause() : music.resume() } label: {
                        Image(systemName: music.playing ? "pause.fill" : "play.fill")
                            .font(.title.weight(.bold))
                            .foregroundStyle(.black)
                            .frame(width: 68, height: 68)
                            .background(.white, in: Circle())
                    }
                    .accessibilityLabel(music.playing ? "Pause" : "Play")
                    Spacer()
                    Button { music.advance() } label: { Image(systemName: "forward.fill").font(.title) }
                        .accessibilityLabel("Next track")
                    Spacer()
                    Button { _ = music.command(["op": "repeat"]) } label: {
                        Image(systemName: music.repeatMode == "one" ? "repeat.1" : "repeat").font(.title3)
                    }
                    .foregroundStyle(music.repeatMode == "off" ? .white : Theme.accent)
                    .accessibilityLabel("Repeat \(music.repeatMode)")
                }
                .buttonStyle(.plain)
                HStack(spacing: 12) {
                    Image(systemName: "speaker.fill").font(.footnote).foregroundStyle(.secondary).accessibilityHidden(true)
                    Slider(value: Binding(get: { music.volume }, set: { music.setVolume($0) }), in: 0...1)
                        .tint(.white)
                        .accessibilityLabel("Volume")
                    AirPlayButton().frame(width: 44, height: 44).accessibilityLabel("Audio output")
                    Button { showQueue = true } label: {
                        Image(systemName: "list.bullet").font(.title3).frame(width: 44, height: 44)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Play queue")
                    .accessibilityIdentifier("open-queue")
                }
                Spacer(minLength: 12)
            }
            .padding(.horizontal, 24)
            .padding(.top, 8)
        }
        .foregroundStyle(.white)
        .background(LinearGradient(colors: [tint.opacity(0.9), .black], startPoint: .top, endPoint: .center).ignoresSafeArea())
        .presentationDragIndicator(.visible)
        .sheet(isPresented: $showQueue) { NativeQueueSheet().environmentObject(music) }
        .sheet(item: $playlistSelection) { selection in PlaylistPickerSheet(selection: selection).environmentObject(music) }
        .sheet(item: $tagEditing) { selection in TagEditorSheet(song: selection.song).environmentObject(music) }
    }

    /// The playing song's actions, like a track row's menu.
    var songMenu: some View {
        let song = music.currentSongRow
        let library = isLibrarySong(song)
        return Menu {
            if !song.isEmpty {
                Button { playlistSelection = PlaylistSelection(song: song) } label: { Label("Add to playlist", systemImage: "text.badge.plus") }
            }
            if let album = albumLink(song) {
                Button { goTo("albums", album) } label: { Label("Go to album", systemImage: "opticaldisc") }
            }
            if let artist = artistLink(song) {
                Button { goTo("artists", artist) } label: { Label("Go to artist", systemImage: "person.crop.circle") }
            }
            if library {
                Button { music.action("download", song) } label: { Label("Download", systemImage: "arrow.down.circle") }
                Button { tagEditing = TagSelection(song: song) } label: { Label("Edit tags…", systemImage: "tag") }
            }
        } label: {
            Image(systemName: "ellipsis").font(.title3.weight(.semibold)).frame(width: 44, height: 44)
        }
        .disabled(song.isEmpty)
        .accessibilityLabel("Song actions")
        .accessibilityIdentifier("now-playing-actions")
    }
}

/// A row label in the Library tab: icon tile, title and a short description.
struct LibraryRow: View {
    let title: String
    let subtitle: String
    let kind: ArtKind
    var symbol: String? = nil
    var body: some View {
        HStack(spacing: 12) {
            ArtworkView(item: ["name": title], kind: kind, size: 52, symbol: symbol)
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(.body.weight(.medium))
                Text(subtitle).font(.subheadline).foregroundStyle(.secondary)
            }
        }
        .padding(.vertical, 2)
    }
}
