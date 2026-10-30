import XCTest

final class Screenshots: XCTestCase {
    override func setUpWithError() throws {
        continueAfterFailure = false
    }
    func capture(_ name: String, app: XCUIApplication) {
        let shot = XCTAttachment(screenshot: app.screenshot())
        shot.name = "native-\(name)"
        shot.lifetime = .keepAlways
        add(shot)
    }
    func select(_ tab: String, title: String, app: XCUIApplication) {
        let button = app.tabBars.buttons[tab]
        XCTAssertTrue(button.waitForExistence(timeout: 10))
        XCTAssertTrue(button.isHittable, "The mini player must not cover the tab bar")
        button.tap()
        XCTAssertTrue(app.navigationBars[title].waitForExistence(timeout: 10))
        let settled = NSPredicate(format: "selected == true")
        expectation(for: settled, evaluatedWith: button)
        waitForExpectations(timeout: 10)
    }
    func openCollectionActions(app: XCUIApplication) {
        let actions = app.navigationBars.buttons["collection-actions"]
        XCTAssertTrue(actions.waitForExistence(timeout: 10))
        XCTAssertTrue(actions.isHittable, "Collection actions must expose a hittable toolbar control")
        actions.tap()
        XCTAssertTrue(app.navigationBars["Collection actions"].waitForExistence(timeout: 10))
    }
    /// The loopback fixture server's richer management data (`--management-fixtures`) is optional;
    /// set TEST_RUNNER_ZAY_MANAGEMENT_FIXTURES=1 when CI starts the server with that flag.
    var managementFixtures: Bool { ProcessInfo.processInfo.environment["ZAY_MANAGEMENT_FIXTURES"] == "1" }
    func launchFixtures() -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments = ["--fixtures"]
        app.launchEnvironment["ZAY_FIXTURE_SERVER"] = "http://127.0.0.1:18765"
        app.launch()
        return app
    }
    /// Lists and forms are lazy. Use small scrolls in both directions instead of
    /// assuming off-screen rows already exist in the accessibility hierarchy.
    func reveal(_ element: XCUIElement, app: XCUIApplication) {
        // Rows being re-created (e.g. after a badge update) briefly have no frame; asking
        // hittability then fails the test outright, so require an on-screen frame first.
        func visible() -> Bool {
            guard element.exists else { return false }
            let frame = element.frame
            return !frame.isEmpty && app.windows.firstMatch.frame.contains(CGPoint(x: frame.midX, y: frame.midY)) && element.isHittable
        }
        for upward in [true, false] {
            for _ in 0..<10 {
                if visible() { return }
                let start = app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: upward ? 0.72 : 0.4))
                let end = app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: upward ? 0.4 : 0.72))
                start.press(forDuration: 0.05, thenDragTo: end)
            }
        }
        XCTAssertTrue(element.waitForExistence(timeout: 2) && visible(), "The native control must be visible after scrolling")
    }
    func tapBack(app: XCUIApplication) {
        let back = app.navigationBars.buttons["catalog-back"]
        XCTAssertTrue(back.waitForExistence(timeout: 10), "In-place collections must offer Back")
        back.tap()
    }
    func openQueueSheet(app: XCUIApplication) {
        let miniPlayer = app.buttons["mini-player"]
        XCTAssertTrue(miniPlayer.waitForExistence(timeout: 10))
        miniPlayer.tap()
        let queue = app.buttons["open-queue"]
        XCTAssertTrue(queue.waitForExistence(timeout: 10))
        queue.tap()
        XCTAssertTrue(app.navigationBars["Play queue"].waitForExistence(timeout: 10))
    }
    func testCollectionNavigationAndSearch() {
        let app = launchFixtures()
        select("Home", title: "Home", app: app)
        let shortcut = app.buttons["shortcut-fixture-album-1"]
        XCTAssertTrue(shortcut.waitForExistence(timeout: 10), "Home must offer recent collection shortcuts")
        XCTAssertTrue(app.buttons["shortcut-play-fixture-album-1"].exists, "Home tiles play without opening")
        shortcut.tap()
        XCTAssertTrue(app.navigationBars["Fixture album 1"].waitForExistence(timeout: 10))
        XCTAssertTrue(app.buttons["row-fixture-1"].waitForExistence(timeout: 10))
        XCTAssertTrue(app.buttons["album-save"].exists, "Albums offer Save to Your Library")
        capture("AlbumCollection", app: app)
        tapBack(app: app)
        XCTAssertTrue(app.navigationBars["Home"].waitForExistence(timeout: 10))
        XCTAssertTrue(app.buttons["row-fixture-1"].waitForExistence(timeout: 10), "Back must restore Home's shelves")

        select("Library", title: "Your Library", app: app)
        app.staticTexts["Artists"].tap()
        XCTAssertTrue(app.navigationBars["Artists"].waitForExistence(timeout: 10))
        let artist = app.buttons["row-fixture-artist"]
        XCTAssertTrue(artist.waitForExistence(timeout: 10))
        artist.tap()
        XCTAssertTrue(app.navigationBars["ZayMusic"].waitForExistence(timeout: 10))
        XCTAssertTrue(app.buttons["artist-follow"].waitForExistence(timeout: 10))
        let album = app.buttons["row-fixture-album-1"]
        XCTAssertTrue(album.waitForExistence(timeout: 10))
        capture("ArtistCollection", app: app)
        album.tap()
        XCTAssertTrue(app.navigationBars["Fixture album 1"].waitForExistence(timeout: 10))
        tapBack(app: app)
        XCTAssertTrue(app.navigationBars["ZayMusic"].waitForExistence(timeout: 10))
        tapBack(app: app)
        XCTAssertTrue(app.navigationBars["Artists"].waitForExistence(timeout: 10))
        artist.press(forDuration: 1)
        XCTAssertTrue(app.buttons["Unfollow"].waitForExistence(timeout: 10))
        XCTAssertFalse(app.buttons["Download album"].exists, "Artists must not offer per-song downloads")
        XCTAssertFalse(app.buttons["Add to queue"].exists, "Artist rows are not songs")
        capture("ArtistMenu", app: app)
        app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.6)).tap()

        expectation(for: NSPredicate(format: "exists == false"), evaluatedWith: app.buttons["Unfollow"])
        waitForExpectations(timeout: 10)
        select("Search", title: "Search", app: app)
        let field = app.textFields["Search or paste a YouTube URL"]
        XCTAssertTrue(field.waitForExistence(timeout: 10))
        field.tap()
        field.typeText("Fixture\n")
        let song = app.buttons["row-fixture-2"]
        XCTAssertTrue(song.waitForExistence(timeout: 10))
        capture("SearchResults", app: app)
        // Chips narrow the same results without searching again.
        app.buttons["Albums"].tap()
        if managementFixtures {
            XCTAssertTrue(app.buttons["row-fixture-album-1"].waitForExistence(timeout: 10), "The Albums filter exposes the mixed collection results")
            capture("SearchAlbums", app: app)
        }
        expectation(for: NSPredicate(format: "exists == false"), evaluatedWith: song)
        waitForExpectations(timeout: 10)
        app.buttons["All"].tap()
        XCTAssertTrue(song.waitForExistence(timeout: 10))
        song.tap()
        openQueueSheet(app: app)
        XCTAssertTrue(app.buttons["queue-row-fixture-3"].waitForExistence(timeout: 10))
        XCTAssertFalse(app.buttons["queue-row-fixture-album-1"].exists, "Album rows must never be queued as songs")
        XCTAssertFalse(app.buttons["queue-row-fixture-artist"].exists, "Artist rows must never be queued as songs")
    }
    func testManagementScreens() {
        let app = launchFixtures()
        select("Settings", title: "Settings", app: app)
        let server = app.textFields["server-address"]
        XCTAssertTrue(server.waitForExistence(timeout: 10))
        XCTAssertTrue(app.sliders["settings-volume"].exists)
        XCTAssertTrue(app.buttons["Cycle repeat"].exists)
        // Place the cursor at the end, clear the address and finish editing with Return.
        server.coordinate(withNormalizedOffset: CGVector(dx: 0.95, dy: 0.5)).tap()
        let existing = server.value as? String ?? ""
        server.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: existing.count) + "https://fake-user:fake-secret@example.invalid\n")
        app.buttons["Save server address"].tap()
        let message = app.staticTexts["settings-message"]
        XCTAssertTrue(message.waitForExistence(timeout: 10))
        XCTAssertEqual(message.label, "Use an HTTP(S) URL without credentials.", "Credentialed server addresses are never saved")
        capture("SettingsRejectedServer", app: app)
        reveal(app.buttons["Verify connection"], app: app)
        if managementFixtures {
            let browser = app.descendants(matching: .any)["managed-browser-status"].firstMatch
            XCTAssertTrue(browser.waitForExistence(timeout: 10))
            XCTAssertEqual(browser.value as? String, "Fixture browser")
        }
        let connect = app.buttons["Connect Google API"]
        reveal(connect, app: app)
        if !managementFixtures {
            XCTAssertFalse(connect.isEnabled, "OAuth needs a server-side client first")
        } else {
            connect.tap()
            XCTAssertTrue(app.staticTexts["oauth-code"].waitForExistence(timeout: 10), "The device code is shown while authorization is pending")
            XCTAssertTrue(app.buttons["Get a new code"].exists)
            app.buttons["oauth-cancel"].tap()
            XCTAssertTrue(connect.waitForExistence(timeout: 10), "Cancelling sign-in returns to Connect")
        }
        capture("SettingsYouTube", app: app)
        let paste = app.buttons["Paste cookies.txt contents"]
        reveal(paste, app: app)
        paste.tap()
        let cookies = app.textViews["cookie-input"]
        XCTAssertTrue(cookies.waitForExistence(timeout: 10))
        let connectCookies = app.buttons["Connect YouTube Music"]
        reveal(connectCookies, app: app)
        XCTAssertFalse(connectCookies.isEnabled, "Empty cookie input cannot be sent")
        XCTAssertTrue(app.buttons["cookie-file"].exists, "cookies.txt can also be chosen from Files")
        reveal(cookies, app: app)
        cookies.tap()
        cookies.typeText("# Netscape HTTP Cookie File\nfixture.invalid TRUE / TRUE 0 FAKE fake-value")
        let done = app.toolbars.buttons["Done"].firstMatch
        XCTAssertTrue(done.waitForExistence(timeout: 10), "The multiline editor must offer a keyboard Done button")
        done.tap()
        reveal(connectCookies, app: app)
        XCTAssertTrue(connectCookies.isEnabled)
        connectCookies.tap()
        let cleared = NSPredicate { _, _ in !cookies.exists || (cookies.value as? String ?? "").isEmpty }
        expectation(for: cleared, evaluatedWith: cookies)
        waitForExpectations(timeout: 10)
        let downloads = app.buttons["Manage downloads"]
        reveal(downloads, app: app)
        downloads.tap()
        XCTAssertTrue(app.navigationBars["Downloads"].waitForExistence(timeout: 10))
        app.navigationBars.buttons["Settings"].tap()

        select("Library", title: "Your Library", app: app)
        for page in ["Jobs", "Imports", "YouTube Playlists"] {
            let link = app.staticTexts[page]
            reveal(link, app: app)
            link.tap()
            XCTAssertTrue(app.navigationBars[page].waitForExistence(timeout: 10))
            if managementFixtures {
                switch page {
                case "Jobs":
                    XCTAssertTrue(app.staticTexts["Fixture failed job"].waitForExistence(timeout: 10))
                    XCTAssertTrue(app.buttons["Retry"].exists)
                    app.buttons["job-filter-attention"].tap()
                    expectation(for: NSPredicate(format: "exists == false"), evaluatedWith: app.staticTexts["Fixture running job"])
                    waitForExpectations(timeout: 10)
                    XCTAssertTrue(app.staticTexts["Fixture failed job"].exists, "Needs attention keeps failed jobs")
                    app.buttons["Dismiss"].firstMatch.tap()
                    XCTAssertTrue(app.buttons["Dismiss job"].waitForExistence(timeout: 10), "Dismissing a job asks for confirmation")
                    app.buttons["Dismiss job"].tap()
                case "Imports":
                    XCTAssertTrue(app.staticTexts["Fixture import"].waitForExistence(timeout: 10))
                    XCTAssertTrue(app.buttons["Open playlist"].exists, "Imports open their linked playlist")
                    XCTAssertTrue(app.buttons["Show failed jobs"].exists, "Imports with failures link to Jobs")
                    app.buttons["Track details"].firstMatch.tap()
                    XCTAssertTrue(app.staticTexts["Fixture import track"].waitForExistence(timeout: 10))
                    capture("ImportDetails", app: app)
                    app.buttons["Done"].tap()
                default:
                    XCTAssertTrue(app.staticTexts["Fixture YouTube playlist"].waitForExistence(timeout: 10))
                    XCTAssertTrue(app.buttons["Import playlist"].exists)
                }
            }
            capture(page.replacingOccurrences(of: " ", with: ""), app: app)
            app.navigationBars.buttons["Your Library"].tap()
            XCTAssertTrue(app.navigationBars["Your Library"].waitForExistence(timeout: 10))
        }
    }
    func testQueueSheetPreservesUnderlyingTab() {
        let app = launchFixtures()
        select("Home", title: "Home", app: app)
        let shortcut = app.buttons["shortcut-fixture-album-1"]
        XCTAssertTrue(shortcut.waitForExistence(timeout: 10))
        shortcut.tap()
        XCTAssertTrue(app.navigationBars["Fixture album 1"].waitForExistence(timeout: 10))
        let first = app.buttons["row-fixture-1"]
        XCTAssertTrue(first.waitForExistence(timeout: 10))
        first.tap()
        openQueueSheet(app: app)
        XCTAssertTrue(app.otherElements["queue-now-playing"].exists || app.staticTexts["Fixture song 1"].exists)
        let next = app.buttons["queue-row-fixture-2"]
        XCTAssertTrue(next.waitForExistence(timeout: 10), "The album's next song is queued")
        capture("QueueSheet", app: app)
        app.buttons["queue-actions-fixture-2"].tap()
        XCTAssertTrue(app.buttons["Move down"].waitForExistence(timeout: 10))
        app.buttons["Remove from queue"].tap()
        expectation(for: NSPredicate(format: "exists == false"), evaluatedWith: next)
        waitForExpectations(timeout: 10)
        XCTAssertTrue(app.staticTexts["Nothing queued"].waitForExistence(timeout: 10))
        app.navigationBars["Play queue"].buttons["Done"].tap()
        let closePlayer = app.buttons["Close player"]
        XCTAssertTrue(closePlayer.waitForExistence(timeout: 10))
        closePlayer.tap()
        expectation(for: NSPredicate(format: "exists == false"), evaluatedWith: closePlayer)
        waitForExpectations(timeout: 10)
        XCTAssertTrue(app.navigationBars["Fixture album 1"].exists, "Queue edits must not replace the tab's collection")
        XCTAssertTrue(app.buttons["row-fixture-2"].exists, "The album still lists the song removed from the queue")
        XCTAssertTrue(app.navigationBars.buttons["catalog-back"].exists)
    }
    func testMainScreens() {
        let app = XCUIApplication()
        app.launchArguments = ["--fixtures"]
        app.launchEnvironment["ZAY_FIXTURE_SERVER"] = "http://127.0.0.1:18765"
        app.launch()
        select("Home", title: "Home", app: app)
        XCTAssertTrue(app.staticTexts["Fixture song 1"].waitForExistence(timeout: 10))
        capture("Home", app: app)
        // Target the row that owns the context menu, rather than its text child.
        let songForPlaylist = app.buttons["row-fixture-4"]
        reveal(songForPlaylist, app: app)
        songForPlaylist.press(forDuration: 1.5)
        let addToPlaylist = app.buttons["Add to playlist"]
        XCTAssertTrue(addToPlaylist.waitForExistence(timeout: 10))
        addToPlaylist.tap()
        XCTAssertTrue(app.navigationBars["Add to playlist"].waitForExistence(timeout: 10))
        let selectedSong = app.staticTexts["playlist-selected-song"]
        XCTAssertTrue(selectedSong.waitForExistence(timeout: 10))
        XCTAssertEqual(selectedSong.label, "Fixture song 4", "The sheet must receive the selected recording on its first presentation")
        XCTAssertTrue(app.buttons["Fixture playlist"].waitForExistence(timeout: 10))
        capture("PlaylistPicker", app: app)
        app.buttons["Fixture playlist"].tap()
        expectation(for: NSPredicate(format: "exists == false"), evaluatedWith: app.navigationBars["Add to playlist"])
        waitForExpectations(timeout: 10)
        select("Search", title: "Search", app: app)
        XCTAssertTrue(app.textFields["Search or paste a YouTube URL"].waitForExistence(timeout: 10))
        capture("Search", app: app)
        select("Library", title: "Your Library", app: app)
        XCTAssertTrue(app.buttons["Liked Songs"].exists || app.staticTexts["Liked Songs"].exists)
        capture("Library", app: app)
        app.staticTexts["Liked Songs"].tap()
        XCTAssertTrue(app.navigationBars["Liked Songs"].waitForExistence(timeout: 10))
        XCTAssertTrue(app.staticTexts["Fixture song 1"].waitForExistence(timeout: 10))
        capture("Liked", app: app)
        app.navigationBars.buttons["Your Library"].tap()
        app.staticTexts["Queue"].tap()
        XCTAssertTrue(app.navigationBars["Queue"].waitForExistence(timeout: 10))
        let priority = app.staticTexts["Fixture song 3"]
        XCTAssertTrue(priority.waitForExistence(timeout: 10))
        priority.press(forDuration: 1)
        XCTAssertTrue(app.buttons["Move down"].waitForExistence(timeout: 10))
        app.buttons["Remove from queue"].tap()
        expectation(for: NSPredicate(format: "exists == false"), evaluatedWith: priority)
        waitForExpectations(timeout: 10)
        capture("Queue", app: app)
        app.navigationBars.buttons["Your Library"].tap()
        app.staticTexts["Downloads"].tap()
        XCTAssertTrue(app.navigationBars["Downloads"].waitForExistence(timeout: 10))
        let offline = app.staticTexts["Offline fixture"]
        XCTAssertTrue(offline.waitForExistence(timeout: 10))
        capture("Downloads", app: app)
        offline.press(forDuration: 1)
        XCTAssertTrue(app.buttons["Retry download"].waitForExistence(timeout: 10))
        app.buttons["Remove download"].tap()
        expectation(for: NSPredicate(format: "exists == false"), evaluatedWith: offline)
        waitForExpectations(timeout: 10)
        app.navigationBars.buttons["Your Library"].tap()
        app.staticTexts["Playlists"].tap()
        XCTAssertTrue(app.staticTexts["Fixture playlist"].waitForExistence(timeout: 10))
        app.staticTexts["Fixture playlist"].tap()
        XCTAssertTrue(app.navigationBars["Fixture playlist"].waitForExistence(timeout: 10))
        let playlistTrack = app.buttons["row-fixture-2"]
        XCTAssertTrue(playlistTrack.waitForExistence(timeout: 10))
        playlistTrack.press(forDuration: 1)
        app.buttons["Move up in playlist"].tap()
        expectation(for: NSPredicate(format: "value == %@", "Playlist position 1"), evaluatedWith: playlistTrack)
        waitForExpectations(timeout: 10)
        playlistTrack.press(forDuration: 1)
        app.buttons["Remove from playlist"].tap()
        expectation(for: NSPredicate(format: "exists == false"), evaluatedWith: playlistTrack)
        waitForExpectations(timeout: 10)
        capture("PlaylistTracks", app: app)
        select("Home", title: "Home", app: app)
        XCTAssertFalse(app.navigationBars["Fixture playlist"].exists, "A collection header must not leak into Home")
        XCTAssertTrue(app.buttons["row-fixture-1"].waitForExistence(timeout: 10))
        capture("HomeRestored", app: app)
        select("Library", title: "Fixture playlist", app: app)
        XCTAssertTrue(app.buttons["row-fixture-4"].waitForExistence(timeout: 10), "Returning to Library must preserve its edited playlist")
        capture("PlaylistRestored", app: app)
        openCollectionActions(app: app)
        capture("CollectionActions", app: app)
        app.buttons["All playlists"].tap()
        XCTAssertTrue(app.navigationBars["Playlists"].waitForExistence(timeout: 10))
        openCollectionActions(app: app)
        app.buttons["New playlist"].tap()
        let nameField = app.alerts.textFields["Playlist name"]
        XCTAssertTrue(nameField.waitForExistence(timeout: 10))
        nameField.typeText("Native mix")
        app.alerts.buttons["Save"].tap()
        let created = app.staticTexts["Native mix"]
        XCTAssertTrue(created.waitForExistence(timeout: 10))
        created.press(forDuration: 1)
        app.buttons["Rename playlist"].tap()
        let renameField = app.alerts.textFields["Playlist name"]
        XCTAssertTrue(renameField.waitForExistence(timeout: 10))
        renameField.tap()
        renameField.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: "Native mix".count) + "Renamed mix")
        app.alerts.buttons["Save"].tap()
        let renamed = app.staticTexts["Renamed mix"]
        XCTAssertTrue(renamed.waitForExistence(timeout: 10))
        capture("Playlists", app: app)
        renamed.press(forDuration: 1)
        app.buttons["Delete playlist"].tap()
        app.buttons["Delete playlist"].tap()
        expectation(for: NSPredicate(format: "exists == false"), evaluatedWith: renamed)
        waitForExpectations(timeout: 10)
        select("Settings", title: "Settings", app: app)
        XCTAssertTrue(app.buttons["Cycle repeat"].waitForExistence(timeout: 10))
        capture("Settings", app: app)
        select("Home", title: "Home", app: app)
        let firstSong = app.buttons["row-fixture-1"]
        XCTAssertTrue(firstSong.waitForExistence(timeout: 10))
        firstSong.tap()
        let miniPlayer = app.buttons["mini-player"]
        XCTAssertTrue(miniPlayer.waitForExistence(timeout: 10))
        XCTAssertTrue(miniPlayer.isHittable, "The mini player must open the player sheet")
        miniPlayer.tap()
        let closePlayer = app.buttons["Close player"]
        XCTAssertTrue(closePlayer.waitForExistence(timeout: 10))
        XCTAssertTrue(app.staticTexts["Fixture song 1"].waitForExistence(timeout: 10))
        XCTAssertTrue(app.buttons["current-like"].exists, "Library songs can be liked from the player")
        XCTAssertTrue(app.buttons["now-playing-actions"].exists)
        capture("NowPlaying", app: app)
        closePlayer.tap()
        expectation(for: NSPredicate(format: "exists == false"), evaluatedWith: closePlayer)
        waitForExpectations(timeout: 10)
    }
}
