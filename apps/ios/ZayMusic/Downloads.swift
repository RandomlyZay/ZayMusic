import Foundation
import UIKit

func nativeFixtureMode() -> Bool {
    ProcessInfo.processInfo.arguments.contains("--fixtures") || ProcessInfo.processInfo.environment["XCTestConfigurationFilePath"] != nil
}
extension Notification.Name { static let nativeDownloadsChanged = Notification.Name("ZayMusic.downloadsChanged") }

final class NativeEnvironment {
    static let shared = NativeEnvironment()
    let core: NativeCore?
    let downloads: BackgroundDownloads?
    private init() {
        let directory = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0].appendingPathComponent("ZayMusic")
        let server = nativeFixtureMode() ? (ProcessInfo.processInfo.environment["ZAY_FIXTURE_SERVER"] ?? "https://example.invalid") : Self.configuredServer()
        core = try? NativeCore(server: server, directory: directory.path)
        downloads = core.map { BackgroundDownloads(core: $0) }
    }
    /// Older builds could store any text. Never keep an address carrying credentials.
    static func configuredServer(_ defaults: UserDefaults = .standard) -> String {
        guard let stored = defaults.string(forKey: "server") else { return defaultServerAddress }
        if let valid = validatedServerAddress(stored) { return valid }
        defaults.removeObject(forKey: "server")
        return defaultServerAddress
    }
}
final class BackgroundSessionEvents: @unchecked Sendable {
    static let shared = BackgroundSessionEvents()
    private let lock = NSLock()
    private var handlers: [String: () -> Void] = [:]
    func store(_ identifier: String, handler: @escaping () -> Void) { lock.lock(); handlers[identifier] = handler; lock.unlock() }
    func complete(_ identifier: String) {
        lock.lock(); let handler = handlers.removeValue(forKey: identifier); lock.unlock()
        DispatchQueue.main.async { handler?() }
    }
}
final class NativeAppDelegate: NSObject, UIApplicationDelegate {
    func application(_ application: UIApplication, handleEventsForBackgroundURLSession identifier: String, completionHandler: @escaping () -> Void) {
        guard identifier == "io.zaymusic.downloads" else { completionHandler(); return }
        BackgroundSessionEvents.shared.store(identifier, handler: completionHandler)
        if NativeEnvironment.shared.downloads == nil { BackgroundSessionEvents.shared.complete(identifier) }
    }
}

final class BackgroundDownloads: NSObject, URLSessionDownloadDelegate, @unchecked Sendable {
    let core: NativeCore
    private let lock = NSLock()
    private var lastNotification = Date.distantPast
    private var restored = false
    private var waiting: [[String: Any]] = []
    lazy var session: URLSession = {
        let configuration = URLSessionConfiguration.background(withIdentifier: "io.zaymusic.downloads")
        configuration.httpMaximumConnectionsPerHost = 2
        configuration.sessionSendsLaunchEvents = true
        configuration.isDiscretionary = false
        return URLSession(configuration: configuration, delegate: self, delegateQueue: nil)
    }()
    init(core: NativeCore) {
        self.core = core
        super.init()
        session.getAllTasks { tasks in
            var active: [[String: Any]] = []
            for task in tasks {
                let ticket = decodeJSON(task.taskDescription ?? "{}")
                guard let id = ticket["id"] as? String, let generation = ticket["ticket"] as? UInt64,
                      (try? core.downloadActive(id: id, ticket: generation)) == true else { task.cancel(); continue }
                active.append(ticket)
                _ = try? core.beginDownload(id: id, ticket: generation, taskKey: "ios:\(task.taskIdentifier)")
                if task.state == .suspended { task.resume() }
            }
            try? core.recoverDownloads(activeJson: encodeJSON(active))
            self.lock.lock(); self.restored = true; let waiting = self.waiting; self.waiting = []; self.lock.unlock()
            self.enqueueMany(waiting)
            self.changed()
        }
    }
    func enqueueMany(_ songs: [[String: Any]]) { songs.forEach { enqueue($0, announce: false) }; changed() }
    func enqueue(_ song: [String: Any], announce: Bool = true) {
        lock.lock()
        if !restored { waiting.append(song); lock.unlock(); return }
        lock.unlock()
        do {
            let plan = decodeJSON(try core.queueDownload(songJson: encodeJSON(song)))
            guard plan["ready"] as? Bool != true, let text = plan["url"] as? String, let url = URL(string: text),
                  let id = plan["id"] as? String, let ticket = plan["ticket"] as? UInt64 else { return }
            let task = session.downloadTask(with: url)
            task.taskDescription = encodeJSON(["id": id, "ticket": ticket])
            if try core.beginDownload(id: id, ticket: ticket, taskKey: "ios:\(task.taskIdentifier)") { task.resume() }
            else { task.cancel() }
            if announce { changed() }
        } catch { if announce { changed() } }
    }
    func cancel(_ id: String) {
        lock.lock(); waiting.removeAll { $0["id"] as? String == id }; lock.unlock()
        try? core.cancelDownload(id: id)
        session.getAllTasks { tasks in
            for task in tasks where decodeJSON(task.taskDescription ?? "{}")["id"] as? String == id { task.cancel() }
        }
        changed()
    }
    func remove(_ id: String) { cancel(id); try? core.deleteDownload(id: id); changed() }
    func removeAll() {
        lock.lock(); waiting = []; lock.unlock()
        try? core.deleteAllDownloads()
        session.getAllTasks { tasks in tasks.forEach { $0.cancel() } }
        changed()
    }
    func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask, didWriteData bytesWritten: Int64, totalBytesWritten: Int64, totalBytesExpectedToWrite: Int64) {
        let value = decodeJSON(downloadTask.taskDescription ?? "{}")
        if let id = value["id"] as? String, let ticket = value["ticket"] as? UInt64 {
            try? core.downloadProgress(id: id, ticket: ticket, received: UInt64(max(0, totalBytesWritten)), total: UInt64(max(0, totalBytesExpectedToWrite)))
            changed(force: false)
        }
    }
    func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask, didFinishDownloadingTo location: URL) {
        let value = decodeJSON(downloadTask.taskDescription ?? "{}")
        guard let id = value["id"] as? String, let ticket = value["ticket"] as? UInt64 else { return }
        let mime = downloadTask.response?.mimeType ?? ""
        guard let response = downloadTask.response as? HTTPURLResponse, (200...299).contains(response.statusCode), !mime.contains("json"), !mime.contains("text/html") else {
            try? core.failDownload(id: id, ticket: ticket, reason: "http"); changed(); return
        }
        do { try core.completeDownload(id: id, ticket: ticket, temporaryFile: location.path) }
        catch { try? core.failDownload(id: id, ticket: ticket, reason: "storage") }
        changed()
    }
    func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        guard error != nil else { return }
        let value = decodeJSON(task.taskDescription ?? "{}")
        if let id = value["id"] as? String, let ticket = value["ticket"] as? UInt64 {
            try? core.failDownload(id: id, ticket: ticket, reason: "transport")
        }
        changed()
    }
    func urlSessionDidFinishEvents(forBackgroundURLSession session: URLSession) {
        if let identifier = session.configuration.identifier { BackgroundSessionEvents.shared.complete(identifier) }
    }
    private func changed(force: Bool = true) {
        lock.lock()
        if !force && Date().timeIntervalSince(lastNotification) < 0.25 { lock.unlock(); return }
        lastNotification = Date(); lock.unlock()
        NotificationCenter.default.post(name: .nativeDownloadsChanged, object: nil)
    }
}
