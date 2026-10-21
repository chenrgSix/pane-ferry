import AppKit

@main
struct AutomaticReconnectSmoke {
    static func main() throws {
        _ = NSApplication.shared
        let root = URL(fileURLWithPath: CommandLine.arguments[1])
        let board = NSPasteboard.withUniqueName(); defer { board.releaseGlobally() }
        let options = Options(host: "fixture.invalid", user: "fixture", port: 22, app: nil, session: "retained")
        let app = AppDelegate(options: options, clipboard: board)
        app.makeTransport = { SSHTransport(executable: root.appendingPathComponent("fake-ssh")) }
        defer {
            app.applicationWillTerminate(Notification(name: NSApplication.willTerminateNotification))
            for host in app.windows.values { host.closeLocally() }
        }
        func until(_ condition: () -> Bool) {
            let deadline = Date().addingTimeInterval(8)
            while !condition(), Date() < deadline {
                RunLoop.current.run(until: Date().addingTimeInterval(0.01)); app.pump()
            }
            precondition(condition(), "automatic reconnect did not reach expected state")
        }
        app.startConnection()
        until { app.windows[42] != nil }
        RunLoop.current.run(until: Date().addingTimeInterval(0.03)); app.pump()
        let original = app.windows[42]!
        original.fixedPixels = (800, 600)
        let frame = original.window.frame
        app.clipboardDeferred = [["type":"input.text.commit", "window_id":UInt32(42), "text":"stale queued input"]]
        app.clipboardBusy = true
        try Data().write(to: root.appendingPathComponent("cut"))
        until { app.recovering && !app.connected }
        precondition(app.windows[42] === original && app.reconnectTimer != nil)
        precondition(!app.clipboardBusy && app.clipboardDeferred.isEmpty && original.surface.transmit == nil)
        app.send(["type":"input.text.commit", "window_id":UInt32(42), "text":"offline input"])
        app.connectionLost() // coalesce repeat failure notifications
        until { app.connected && !app.recovering && app.windows[42]?.revision == 2 }
        precondition(app.windows[42] === original && original.window.frame == frame)
        precondition(original.fixedPixels?.width == 800 && original.fixedPixels?.height == 600)
        precondition(original.surface.transmit != nil && app.reconnectTimer == nil && app.reconnectAttempt == 0)
        app.send(["type":"input.text.commit", "window_id":UInt32(42), "text":"after reconnect"])
        until { ((try? String(contentsOf: root.appendingPathComponent("requests"), encoding: .utf8)) ?? "").contains("after reconnect") }
        let requests = try String(contentsOf: root.appendingPathComponent("requests"), encoding: .utf8)
        let values = try requests.split(separator:"\n").map { try JSONSerialization.jsonObject(with: Data($0.utf8)) as! [String:Any] }
        precondition(values.filter { $0["type"] as? String == "session.resume" }.count == 2)
        precondition(!values.contains { $0["type"] as? String == "app.launch" })
        precondition(!requests.contains("offline input") && !requests.contains("stale queued input"))
        let count = try String(contentsOf: root.appendingPathComponent("connections"), encoding: .utf8)
        precondition(count == "2")
        precondition((0...10).map(AppDelegate.reconnectDelay) == [1,2,4,8,15,15,15,15,15,15,15])
        app.applicationWillTerminate(Notification(name: NSApplication.willTerminateNotification))
        app.connectionLost(); precondition(app.reconnectTimer == nil)

        // Identity selection is per endpoint and never activates a pending key.
        let directory = root.appendingPathComponent(".ssh/lan-remoteapp-keys")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories:true)
        let key = directory.appendingPathComponent("fixture@fixture.invalid-22")
        try Data().write(to:key)
        precondition(SSHIdentity.options(host:options.host, user:options.user, port:22, home:root) == ["-S", "~/.ssh/lan-remoteapp-%C"])
        try Data().write(to:key.appendingPathExtension("enabled"))
        let active = SSHIdentity.options(host:options.host, user:options.user, port:22, home:root)
        precondition(active.contains("~/.ssh/lan-remoteapp-auto-%C") && active.contains(key.path))
        precondition(!SSHIdentity.options(host:options.host, user:options.user, port:2222, home:root).contains(key.path))

        // A lost initial launch response must reuse its request ID. Once a
        // session is known, even --app startup resumes instead of relaunching.
        let launch = AppDelegate(options:Options(host:options.host, user:options.user, port:22, app:"fixture-app", session:nil), clipboard:board)
        var sent:[[String:Any]]=[]; launch.wireSink = { sent.append($0) }
        let hello = Incoming(object:["type":"hello", "capabilities":[]], image:nil)
        launch.receive(hello); launch.ready = false; launch.recovering = true; launch.receive(hello)
        let ids = sent.filter { $0["type"] as? String == "app.launch" }.compactMap { $0["request_id"] as? String }
        precondition(ids.count == 2 && ids[0] == ids[1])
        launch.session = "known"; launch.ready = false; launch.receive(hello)
        precondition(sent.last?["type"] as? String == "session.resume" && sent.last?["session_id"] as? String == "known")
        launch.stopRecovery()
        print("PASS: real transport subprocess loss automatically resumes the same NSWindow/session, resets stream sequence, preserves resolution, drops offline input and stops on quit; restricted identities and launch idempotence")
    }
}
