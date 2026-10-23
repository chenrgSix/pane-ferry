import AppKit

// Opt-in: the paired publisher must expose ONLY a disposable GTK text editor
// under the supplied application ID. No SSH process or system pasteboard is used.
@main struct LiveQUICSmoke {
    static func main() throws {
        _ = NSApplication.shared
        var options = try Options.parse(Array(CommandLine.arguments.dropFirst()))
        options.quality = .smooth // This fixture explicitly verifies H.264.
        let board = NSPasteboard.withUniqueName()
        defer { board.releaseGlobally() }
        let app = AppDelegate(options: options, clipboard: board)
        defer {
            app.applicationWillTerminate(Notification(name: NSApplication.willTerminateNotification))
            for host in app.windows.values { host.closeLocally() }
        }
        func until(_ condition: () -> Bool) {
            let deadline = Date().addingTimeInterval(20)
            while !condition(), Date() < deadline {
                RunLoop.current.run(until: Date().addingTimeInterval(0.01))
                app.pump()
            }
            precondition(condition(), "live QUIC check timed out")
        }
        app.applicationDidFinishLaunching(Notification(name: NSApplication.didFinishLaunchingNotification))
        until { app.ready && app.windows.values.contains { $0.surface.image != nil } }
        let host = app.windows.values.first { $0.surface.image != nil }!
        let session = app.session!
        precondition(host.window.subtitle.contains("H.264"), "expected actual H.264 decoding")
        func send(_ type: String, _ fields: [String: Any] = [:]) {
            var value = fields; value["type"] = type; value["window_id"] = host.id
            app.send(value)
        }
        func key(_ symbol: UInt32, _ pressed: Bool) {
            send("input.key", ["keysym": symbol, "pressed": pressed])
        }
        func selectAll() {
            key(0xffe3, true); key(97, true); key(97, false); key(0xffe3, false)
        }
        func copy(_ expected: String) {
            selectAll()
            send("clipboard.copy", ["request_id": UUID().uuidString])
            until { !app.clipboardBusy && board.string(forType: .string) == expected }
        }
        send("input.text.commit", ["text": "独立 QUIC 中文🙂"])
        copy("独立 QUIC 中文🙂")
        board.clearContents(); board.setString("Mac → Ubuntu 剪贴板", forType: .string)
        send("controller.paste")
        copy("Mac → Ubuntu 剪贴板")
        send("clipboard.cut", ["request_id": UUID().uuidString])
        until { !app.clipboardBusy }
        key(0xff51, true); key(0xff51, false)
        send("input.text.commit", ["text": "剪切后输入正常🙂"])
        copy("剪切后输入正常🙂")
        host.fixedPixels = (960, 640)
        send("window.configure", ["width": 960, "height": 640])
        until { host.surface.image?.width == 960 && host.surface.image?.height == 640 }
        for _ in 0..<3 {
            let previous = host.surface.image
            app.transport.disconnect()
            until { app.recovering }
            until { app.connected && !app.recovering && host.surface.image != nil && host.surface.image !== previous }
            precondition(app.session == session && app.windows[host.id] === host)
            copy("剪切后输入正常🙂")
        }
        send("window.close.request")
        until { app.windows.isEmpty }
        print("PASS: live Ubuntu GTK -> QUIC -> AppKit H.264; Unicode, bidirectional text clipboard, cut, resize, three same-window/session reconnects and remote close; no SSH in receiver")
    }
}
