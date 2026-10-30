import SwiftUI
import UIKit
import UniformTypeIdentifiers

extension Music {
    func setVolume(_ value: Double) {
        guard let core, value.isFinite else { return }
        do {
            try core.setVolume(volume: value)
            volume = core.volume()
            player.volume = (decodeJSON(try core.queueCommand(command: "{}"))["gain"] as? NSNumber)?.floatValue ?? Float(volume)
        } catch { self.error = "Volume could not be saved" }
    }
    /// Envelopes can contain pasted cookies: they are sent once and never stored, logged or echoed.
    func manage(_ command: [String: Any]) async -> Bool {
        guard let core else { return false }
        let envelope = encodeJSON(command)
        do {
            _ = try await Task.detached { try core.managementCommand(command: envelope) }.value
            Task { await checkServerAttention() }
            return true
        } catch { return false }
    }
    /// `extra` adds top-level fields such as `edits` (tag edits) or `playlistId` (album save target).
    @discardableResult
    func library(_ op: String, item: [String: Any], kind: String, extra: [String: Any] = [:]) async -> Bool {
        guard let core else { return false }
        error = ""
        var command: [String: Any] = ["op": op, "kind": kind, "item": item]
        command.merge(extra) { _, new in new }
        let envelope = encodeJSON(command)
        do {
            let result = decodeJSON(try await Task.detached { try core.libraryCommand(command: envelope) }.value)
            if let id = item["id"] as? String, kind == "songs" {
                switch op {
                // The core already removed the song from both queue lanes; restart only if it was playing.
                case "deleteSong": if id == currentSong["id"] as? String { rebuild(start: wantsPlayback) } else { refreshUpcoming() }
                case "save", "unsave": refreshCurrentMetadata(id, patch: ["starred": op == "save" ? "saved" : nil])
                case "editTags": refreshCurrentMetadata(id, patch: (extra["edits"] as? [String: Any] ?? [:]).mapValues { value -> Any? in value is NSNull ? nil : value })
                default: break
                }
            }
            await refreshCatalog()
            notice = libraryNotice(op, kind: kind, result: result)
            return true
        } catch {
            switch op {
            case "deleteSong": self.error = "Song could not be deleted. Check the connection and retry."
            case "editTags": self.error = "Tags could not be saved. Check the connection and retry."
            default: self.error = "Library could not be updated. Check the connection and retry."
            }
            return false
        }
    }
    func libraryNotice(_ op: String, kind: String, result: [String: Any]) -> String {
        switch (op, kind) {
        case ("save", "songs"): return "Added to Liked Songs"
        case ("unsave", "songs"): return "Removed from Liked Songs"
        case ("save", "artists"): return "Following artist"
        case ("unsave", "artists"): return "Unfollowed artist"
        case ("save", _): return "Saved to Your Library"
        case ("unsave", _): return "Removed from Your Library"
        case ("acquireAlbum", _):
            let queued = result["queued"] as? Int ?? 0
            let failed = result["failed"] as? Int ?? 0
            return failed > 0 ? "\(queued) songs queued, \(failed) failed. Open Jobs to retry." : "Album saved. \(queued) songs queued for your library."
        case ("deleteSong", _): return "Deleted from server"
        case ("editTags", _): return "Tags updated"
        default: return "Library updated"
        }
    }
    func refreshCatalog() async {
        guard let core else { return }
        if page == "Collection", let source = collectionContext["kind"] as? String, let id = collectionContext["id"] as? String {
            loadGeneration += 1; let generation = loadGeneration
            do {
                let text = try await Task.detached { try core.openCollection(kind: source, id: id) }.value
                if generation == loadGeneration { applyCatalog(decodeJSON(text)) }
            } catch { if generation == loadGeneration { self.error = "Collection could not be refreshed" } }
        } else { await load(page, query: searchQuery, refresh: true) }
    }
    /// Albums, playlists and Liked Songs become Home shortcuts once played.
    func recordPlayContext() {
        guard let core, page == "Liked Songs" || page == "Collection" else { return }
        try? core.recordContext(contextJson: encodeJSON(currentPlayContext))
    }
    /// Imports and new downloads need a working YouTube Music session; say so where they are managed.
    func checkConnection() async {
        guard let core else { return }
        let text = await Task.detached { try? core.managementStatus() }.value
        guard let text else { return }
        connectionWarning = youtubeAttention(decodeJSON(text)["youtube"] as? [String: Any] ?? [:])
    }
}

/// Stock iOS settings form over the shared core's connection, session and storage contracts.
struct NativeSettings<DownloadsScreen: View>: View {
    @EnvironmentObject var music: Music
    @ViewBuilder let downloads: () -> DownloadsScreen
    @AppStorage("server") private var storedServer = defaultServerAddress
    @State private var server = ""
    @State private var remote = ""
    @State private var status: [String: Any] = [:]
    @State private var statusUnavailable = false
    @State private var cookieText = ""
    @State private var busy = false
    @State private var message = ""
    @State private var confirmDisconnect = false
    @State private var choosingCookieFile = false
    @State private var loaded = false
    @FocusState private var focus: Field?
    private enum Field { case server, remote, cookies }

    private var preferences: [String: Any] { status["preferences"] as? [String: Any] ?? [:] }
    private var youtube: [String: Any] { status["youtube"] as? [String: Any] ?? [:] }
    private var oauth: [String: Any] { youtube["oauth"] as? [String: Any] ?? [:] }
    private var flow: [String: Any] { status["oauthFlow"] as? [String: Any] ?? [:] }
    private var storedBytes: Int64 { ((status["storage"] as? [String: Any])?["readyBytes"] as? NSNumber)?.int64Value ?? Int64(music.storageBytes) }
    private var savedRemote: URL? {
        (preferences["remoteBrowserUrl"] as? String).flatMap { validatedServerAddress($0) }.flatMap { URL(string: $0) }
    }
    private var oauthError: String { status["oauthError"] as? String ?? "" }
    private var sessionState: String {
        guard youtube["configured"] as? Bool == true else { return "Not connected" }
        guard youtube["verified"] as? Bool == true else { return "Needs attention" }
        return youtube["premium"] as? Bool == true ? "Connected · Premium" : "Connected"
    }
    /// The server's managed-browser states in plain words; unknown values pass through.
    private var browserState: String {
        switch youtube["browser"] as? String {
        case nil, "": return "Not set up"
        case "connected": return "Signed in"
        case "signed_out": return "Signed out"
        case "disconnected": return "Offline"
        case let other?: return other
        }
    }
    private var browserHelp: String {
        switch youtube["browser"] as? String {
        case "signed_out": return "Open the remote browser and sign in to YouTube Music once."
        case "disconnected": return "The server's managed browser isn't running. Check the server."
        default: return ""
        }
    }
    private var reloginState: String? {
        guard let state = youtube["lastReloginState"] as? String, !state.isEmpty else { return nil }
        switch state {
        case "reconnected": return "Reconnected automatically"
        case "needs_human": return "Needs you to sign in"
        case "busy": return "In progress"
        case "failed": return "Failed"
        case "disabled": return "Off"
        default: return state.replacingOccurrences(of: "_", with: " ").capitalized
        }
    }

    var body: some View {
        Form {
            if !message.isEmpty {
                Section { Text(message).accessibilityIdentifier("settings-message") }
            }
            Section {
                TextField("Server address", text: $server, prompt: Text(defaultServerAddress))
                    .keyboardType(.URL)
                    .textContentType(.URL)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .focused($focus, equals: .server)
                    .accessibilityIdentifier("server-address")
                if let current = preferences["server"] as? String, !current.isEmpty {
                    LabeledContent("Connected to", value: current)
                }
                Button("Save server address") { saveServer() }
                    .disabled(server.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            } header: { Text("Server") } footer: { Text("Use the server's Tailscale HTTPS address. Restart ZayMusic after changing servers.") }
            Section("Playback") {
                Toggle("Autoplay", isOn: Binding(get: { music.autoplay }, set: { music.setAutoplay($0) })).tint(Theme.accent)
                Slider(value: Binding(get: { music.volume }, set: { music.setVolume($0) }), in: 0...1) {
                    Text("Volume")
                } minimumValueLabel: {
                    Image(systemName: "speaker.fill").accessibilityHidden(true)
                } maximumValueLabel: {
                    Image(systemName: "speaker.wave.3.fill").accessibilityHidden(true)
                }
                .accessibilityLabel("Volume")
                .accessibilityIdentifier("settings-volume")
                Toggle("Shuffle", isOn: Binding(get: { music.shuffleOn }, set: { if $0 != music.shuffleOn { _ = music.command(["op": "shuffle"]) } })).tint(Theme.accent)
                Button("Cycle repeat") { _ = music.command(["op": "repeat"]) }
                LabeledContent("Repeat", value: music.repeatMode == "one" ? "One song" : music.repeatMode.capitalized)
            }
            Section {
                if statusUnavailable && status.isEmpty {
                    Label("Connection status unavailable. Check Tailscale.", systemImage: "exclamationmark.triangle").foregroundStyle(.orange)
                } else if status.isEmpty {
                    HStack(spacing: 10) { ProgressView(); Text("Checking connection…").foregroundStyle(.secondary) }
                } else {
                    LabeledContent("Managed browser", value: browserState)
                        .accessibilityElement(children: .combine)
                        .accessibilityValue(browserState)
                        .accessibilityIdentifier("managed-browser-status")
                    if !browserHelp.isEmpty { Text(browserHelp).font(.footnote).foregroundStyle(.secondary) }
                    LabeledContent("Session", value: sessionState).accessibilityIdentifier("youtube-session")
                    if youtube["selfRelogin"] as? Bool == true {
                        Text("The server reconnects the managed session automatically.").font(.footnote).foregroundStyle(.secondary)
                    }
                    if let state = reloginState { LabeledContent("Last reconnect", value: state) }
                    if let days = (youtube["expiresInDays"] as? NSNumber)?.intValue {
                        LabeledContent("Cookies expire", value: days <= 0 ? "Expired" : "In \(days) day\(days == 1 ? "" : "s")")
                    }
                    if let text = youtube["message"] as? String, !text.isEmpty { Text(text).font(.footnote) }
                }
                TextField("Remote browser URL", text: $remote, prompt: Text("https://server:8446/vnc.html"))
                    .keyboardType(.URL)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .focused($focus, equals: .remote)
                    .accessibilityIdentifier("remote-browser-url")
                Button("Save browser address") { saveRemote() }
                    .disabled(remote.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                if let url = savedRemote {
                    Link(destination: url) { Label("Open remote browser", systemImage: "safari") }
                }
                Button("Verify connection") { perform(["op": "verifyConnection"], success: "Connection checked.") }
                if youtube["configured"] as? Bool == true {
                    Button("Disconnect cookies", role: .destructive) { confirmDisconnect = true }
                }
            } header: { Text("YouTube Music") } footer: { Text("Sign in once in the server's managed browser. ZayMusic never stores YouTube credentials.") }
            .disabled(busy)
            Section {
                LabeledContent("API connection", value: oauth["connected"] as? Bool == true ? "Connected" : "Disconnected")
                    .accessibilityIdentifier("oauth-state")
                if let code = flow["user_code"] as? String, !code.isEmpty {
                    Text(code).font(.title2.monospaced().weight(.semibold)).textSelection(.enabled)
                        .accessibilityLabel("Authorization code \(code)")
                        .accessibilityIdentifier("oauth-code")
                    if let text = flow["verification_url"] as? String, let url = URL(string: text) {
                        Link(destination: url) { Label("Authorize in browser", systemImage: "safari") }
                    }
                    Button { UIPasteboard.general.string = code; message = "Code copied." } label: { Label("Copy code", systemImage: "doc.on.doc") }
                    HStack(spacing: 10) { ProgressView(); Text("Waiting for authorization…").foregroundStyle(.secondary) }
                    Button("Get a new code") { perform(["op": "oauthStart"], success: "New code ready.") }
                    // Cancelling forgets the pending code only; an existing connection is kept.
                    Button("Cancel sign-in", role: .cancel) { perform(["op": "oauthCancel"], success: "Sign-in cancelled.") }
                        .accessibilityIdentifier("oauth-cancel")
                } else if oauth["connected"] as? Bool == true {
                    Button("Disconnect Google API", role: .destructive) { perform(["op": "oauthDisconnect"], success: "Google API disconnected.") }
                } else {
                    Button("Connect Google API") { perform(["op": "oauthStart"], success: "Enter the code to authorize ZayMusic.") }
                        .disabled(oauth["configured"] as? Bool != true)
                    if oauth["configured"] as? Bool != true {
                        Text("Configure the OAuth client on the server first.").font(.footnote).foregroundStyle(.secondary)
                    }
                }
                if !oauthError.isEmpty {
                    Label(oauthError, systemImage: "exclamationmark.triangle.fill").font(.footnote).foregroundStyle(.orange)
                        .accessibilityIdentifier("oauth-error")
                }
            } header: { Text("Google API (OAuth)") } footer: { Text("Sign-in codes stay in memory and expire automatically.") }
            .disabled(busy)
            Section {
                DisclosureGroup("Paste cookies.txt contents") {
                    TextEditor(text: $cookieText)
                        .font(.footnote.monospaced())
                        .frame(minHeight: 140)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .keyboardType(.asciiCapable)
                        .privacySensitive()
                        .focused($focus, equals: .cookies)
                        .accessibilityLabel("Cookies.txt contents")
                        .accessibilityIdentifier("cookie-input")
                    PasteButton(payloadType: String.self) { values in
                        let text = values.joined(separator: "\n")
                        Task { @MainActor in cookieText = text }
                    }
                }
                Button { choosingCookieFile = true } label: { Label("Choose cookies.txt file", systemImage: "doc") }
                    .accessibilityIdentifier("cookie-file")
                Button("Connect YouTube Music") { sendCookies() }
                    .disabled(cookieText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                if !cookieText.isEmpty {
                    Button("Clear pasted cookies", role: .destructive) { cookieText = "" }
                }
            } header: { Text("Advanced — manual cookies") } footer: { Text("Managed sign-in is preferred. Cookies are sent to the server once and never saved in this app.") }
            .disabled(busy)
            Section("Storage") {
                LabeledContent("Downloaded audio", value: ByteCountFormatter.string(fromByteCount: storedBytes, countStyle: .file))
                NavigationLink { downloads() } label: { Label("Manage downloads", systemImage: "arrow.down.circle") }
            }
        }
        .navigationTitle("Settings")
        .scrollDismissesKeyboard(.interactively)
        // Return inserts a line in the multiline cookie editor, so the keyboard needs Done.
        .toolbar {
            ToolbarItemGroup(placement: .keyboard) {
                Spacer()
                Button("Done") { focus = nil }
            }
        }
        .confirmationDialog("Disconnect YouTube cookies?", isPresented: $confirmDisconnect, titleVisibility: .visible) {
            Button("Disconnect", role: .destructive) { perform(["op": "disconnectCookies"], success: "Cookies disconnected.") }
        } message: { Text("Acquisitions that need YouTube Music sign-in stop until you reconnect.") }
        .task {
            if !loaded {
                server = storedServer
                if let core = music.core { remote = decodeJSON((try? core.preferences()) ?? "{}")["remoteBrowserUrl"] as? String ?? "" }
                loaded = true
            }
            // Status refreshes also advance an OAuth device flow; the core rate-limits its polls.
            while !Task.isCancelled {
                await refresh()
                do { try await Task.sleep(nanoseconds: 5_000_000_000) } catch { break }
            }
        }
        .onDisappear { cookieText = "" }
        .fileImporter(isPresented: $choosingCookieFile, allowedContentTypes: [.plainText, .text, .data]) { result in
            // The file is read once into the form; it is never copied, logged or kept.
            guard case .success(let url) = result else { message = "No file chosen."; return }
            let scoped = url.startAccessingSecurityScopedResource()
            defer { if scoped { url.stopAccessingSecurityScopedResource() } }
            guard let size = (try? url.resourceValues(forKeys: [.fileSizeKey]))?.fileSize, size <= 1_000_000,
                  let data = try? Data(contentsOf: url), let text = String(data: data, encoding: .utf8), !text.isEmpty else {
                message = "Choose a cookies.txt file exported from your browser."
                return
            }
            cookieText = text
            message = "Cookie file loaded. Tap Connect YouTube Music to send it."
        }
    }
    func refresh() async {
        guard let core = music.core else { statusUnavailable = true; return }
        do {
            status = decodeJSON(try await Task.detached { try core.managementStatus() }.value)
            statusUnavailable = false
        } catch { statusUnavailable = true }
    }
    func saveServer() {
        // Never persist an address carrying credentials; the core enforces the same rule.
        guard let address = validatedServerAddress(server) else { message = "Use an HTTP(S) URL without credentials."; return }
        do {
            try music.core?.setPreference(key: "server", value: address)
            storedServer = address
            server = address
            message = "Saved. Restart ZayMusic to connect to the new server."
        } catch { message = "Address could not be saved." }
    }
    func saveRemote() {
        guard let address = validatedServerAddress(remote) else { message = "Use an HTTP(S) URL without credentials, query or fragment."; return }
        do {
            try music.core?.setPreference(key: "remoteBrowserUrl", value: address)
            remote = address
            message = "Browser address saved."
            Task { await refresh() }
        } catch { message = "Use an HTTP(S) URL without credentials, query or fragment." }
    }
    func sendCookies() {
        focus = nil
        let cookies = cookieText
        busy = true
        message = ""
        Task {
            let ok = await music.manage(["op": "saveCookies", "cookies": cookies])
            if ok { cookieText = "" }
            await refresh()
            busy = false
            message = ok ? "Cookies sent. Verify the connection to confirm." : "Cookies were not accepted. Check the contents and retry."
        }
    }
    func perform(_ command: [String: Any], success: String) {
        busy = true
        message = ""
        Task {
            let ok = await music.manage(command)
            await refresh()
            busy = false
            message = ok ? success : "Action failed. Check the connection and retry."
        }
    }
}

/// One ZayFetch job, playlist import or YouTube Music playlist with its shared-core actions.
struct ManagementRow: View {
    @EnvironmentObject var music: Music
    let item: [String: Any]
    let kind: String
    @State private var showDetails = false
    @State private var confirmDelete = false
    var id: String { item["id"] as? String ?? item["playlistId"] as? String ?? "" }
    var title: String { item["name"] as? String ?? item["title"] as? String ?? "Download" }
    var status: String { item["status"] as? String ?? "" }
    var active: Bool { ["queued", "running", "downloading"].contains(status) }
    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title).font(.headline)
            if kind == "jobs" {
                Text([status.capitalized, item["stage"] as? String ?? ""].filter { !$0.isEmpty }.joined(separator: " · ")).font(.caption).foregroundStyle(.secondary)
                if active, let progress = item["progress"] as? Double { ProgressView(value: min(1, max(0, progress))).tint(Theme.accent) }
                if let error = item["error"] as? String, !error.isEmpty { Text(error).font(.caption).foregroundStyle(.orange) }
            } else if kind == "imports" {
                Text("\(item["done"] as? Int ?? 0) / \(item["total"] as? Int ?? 0) complete · \(item["failed"] as? Int ?? 0) failed").font(.caption).foregroundStyle(.secondary)
            } else {
                let count = item["count"] as? Int ?? 0
                Text("\(count) song\(count == 1 ? "" : "s")").font(.caption).foregroundStyle(.secondary)
            }
            HStack(spacing: 16) {
                if kind == "jobs" {
                    if ["failed", "cancelled"].contains(status) { Button("Retry") { run("retryJob", success: "Retrying.") } }
                    if active { Button("Cancel") { run("cancelJob", success: "Cancelled.") } }
                    else { Button("Dismiss", role: .destructive) { confirmDelete = true } }
                } else if kind == "imports" {
                    if let playlist = item["playlistId"] as? String, !playlist.isEmpty {
                        Button("Open playlist") { music.openCollection("playlists", row: ["id": playlist, "name": title]) }
                    }
                    Button("Track details") { showDetails = true }
                    if item["failed"] as? Int ?? 0 > 0 { Button("Show failed") { music.showFailedJobs() }.accessibilityLabel("Show failed jobs") }
                    Menu {
                        Button { run("pullImport", success: "Pulling new songs.") } label: { Label("Pull new songs", systemImage: "arrow.down.circle") }
                        Button { run("syncImport", success: "Import refreshed.") } label: { Label("Refresh status", systemImage: "arrow.clockwise") }
                    } label: { Image(systemName: "ellipsis.circle") }
                    .accessibilityLabel("More import actions")
                } else {
                    Button("Import playlist") { run("importPlaylist", success: "Import started. Track progress in Imports.") }
                }
            }.buttonStyle(.borderless).font(.subheadline)
        }
        .padding(.vertical, 4)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("\(kind)-\(id)")
        .confirmationDialog("Dismiss this job?", isPresented: $confirmDelete, titleVisibility: .visible) {
            Button("Dismiss job", role: .destructive) { run("deleteJob", success: "Job dismissed.") }
        }
        .sheet(isPresented: $showDetails) {
            NavigationStack {
                List(Array((item["items"] as? [[String: Any]] ?? []).enumerated()), id: \.offset) { _, track in
                    VStack(alignment: .leading, spacing: 2) {
                        Text(track["title"] as? String ?? "Track")
                        Text([(track["status"] as? String ?? "").capitalized, track["artist"] as? String ?? ""].filter { !$0.isEmpty }.joined(separator: " · "))
                            .font(.caption).foregroundStyle(.secondary)
                        if let error = track["error"] as? String, !error.isEmpty { Text(error).font(.caption).foregroundStyle(.orange) }
                    }
                }
                .navigationTitle(title)
                .navigationBarTitleDisplayMode(.inline)
                .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { showDetails = false } } }
            }
        }
    }
    func run(_ op: String, success: String) {
        let page = music.page
        Task {
            guard await music.manage(["op": op, "id": id]) else {
                if music.page == page { music.error = "Action failed. Check the connection and retry." }
                return
            }
            guard music.page == page else { return }
            music.error = ""
            music.notice = success
            await music.refreshCatalog()
        }
    }
}
