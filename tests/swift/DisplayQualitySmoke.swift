import AppKit

@main
struct DisplayQualitySmoke {
    static func main() throws {
        _ = NSApplication.shared
        let root = URL(fileURLWithPath: CommandLine.arguments[1])
        let suite = "io.paneferry.quality-test.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let settings = DisplayQualitySettings(defaults: defaults)
        precondition(settings.selected == .clarity)
        defaults.set("invalid", forKey: "quality")
        precondition(settings.selected == .clarity)
        let args = ["--native-peer", "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", "--native-state", root.path,
                    "--native-helper", root.appendingPathComponent("publisher").path, "--session", "retained"]
        let options = try Options.parse(args)
        precondition(options.quality == nil)
        let smoothOptions = try Options.parse(args + ["--quality", "h264"])
        let clarityOptions = try Options.parse(args + ["--quality", "png"])
        precondition(smoothOptions.quality == .smooth && clarityOptions.quality == .clarity)
        do { _ = try Options.parse(args + ["--quality", "jpeg"]); preconditionFailure("invalid quality accepted") }
        catch HostError.message { }
        let board = NSPasteboard.withUniqueName(); defer { board.releaseGlobally() }
        let app = AppDelegate(options: options, clipboard: board, qualitySettings: settings)
        let other = AppDelegate(options: options, clipboard: board, qualitySettings: settings)
        defer {
            app.applicationWillTerminate(Notification(name: NSApplication.willTerminateNotification))
            for host in app.windows.values { host.closeLocally() }
        }
        func until(_ condition: () -> Bool) {
            let deadline = Date().addingTimeInterval(7)
            while !condition(), Date() < deadline {
                RunLoop.current.run(until: Date().addingTimeInterval(0.01)); app.pump()
            }
            precondition(condition(), "quality switch timed out")
        }
        func requests() -> [[String: Any]] {
            (try? String(contentsOf: root.appendingPathComponent("requests"), encoding: .utf8))?.split(separator: "\n").compactMap {
                (try? JSONSerialization.jsonObject(with: Data($0.utf8))) as? [String: Any]
            } ?? []
        }
        func settled(_ connection: Int) -> Bool {
            app.connected && !app.recovering && app.windows[42]?.revision == UInt64(connection) &&
                app.windows[42]?.surface.image != nil && app.windows[43]?.surface.image != nil && app.lastSequence >= 3
        }
        app.applicationDidFinishLaunching(Notification(name: NSApplication.didFinishLaunchingNotification))
        until { settled(1) }
        // Let the first-window screen fitting finish before choosing a viewport.
        RunLoop.current.run(until: Date().addingTimeInterval(0.1)); app.pump()
        let host = app.windows[42]!, child = app.windows[43]!
        precondition(host.mediaMode?.contains("PNG 无损") == true)
        // Inspect encoded components directly, before AppKit's display color conversion.
        let bitmap = NSBitmapImageRep(cgImage: host.surface.image!)
        var pixel = [Int](repeating: 0, count: bitmap.samplesPerPixel)
        bitmap.getPixel(&pixel, atX: 20, y: 20)
        precondition(Array(pixel.prefix(3)) == [255, 0, 0])
        host.fixedPixels = (640, 480)
        host.window.setContentSize(NSSize(width: 900, height: 600))
        let frame = host.window.frame, rect = host.surface.imageRect
        func pointer() -> [Int] {
            let count = requests().filter { $0["type"] as? String == "input.pointer" }.count
            let point = host.surface.convert(NSPoint(x: rect.midX, y: rect.midY), to: nil)
            for type in [NSEvent.EventType.leftMouseDown, .leftMouseUp] {
                let event = NSEvent.mouseEvent(with: type, location: point, modifierFlags: [], timestamp: 0,
                    windowNumber: host.window.windowNumber, context: nil, eventNumber: 0, clickCount: 1, pressure: 0)!
                if type == .leftMouseDown { host.surface.mouseDown(with: event) } else { host.surface.mouseUp(with: event) }
            }
            until { requests().filter { $0["type"] as? String == "input.pointer" }.count >= count + 2 }
            let last = requests().last { $0["type"] as? String == "input.pointer" }!
            return [last["x"] as! Int, last["y"] as! Int]
        }
        let coordinates = pointer(); precondition(coordinates == [320, 240])
        func preserved() {
            precondition(app.session == "retained" && app.windows[42] === host && app.windows[43] === child)
            precondition(host.window.frame == frame && host.surface.imageRect == rect)
            precondition(child.window.parent === host.window && pointer() == coordinates)
        }
        let item = NSMenuItem(title: DisplayQuality.smooth.title, action: #selector(AppDelegate.selectDisplayQuality(_:)), keyEquivalent: "")
        item.representedObject = "h264"
        app.clipboardBusy = true
        precondition(!app.validateMenuItem(item))
        app.selectDisplayQuality(item); precondition(app.displayQuality == .clarity)
        app.clipboardBusy = false
        app.selectDisplayQuality(item)
        precondition(app.recovering && host.surface.transmit == nil)
        app.send(["type": "input.text.commit", "text": "must not replay", "window_id": UInt32(42)])
        until { settled(2) }
        preserved()
        precondition(host.fixedPixels?.width == 640 && host.mediaMode == "H.264 视频")
        precondition(settings.selected == .smooth && other.displayQuality == .clarity)
        precondition(AppDelegate(options: options, clipboard: board, qualitySettings: settings).displayQuality == .smooth)
        precondition(app.validateMenuItem(item) && item.state == .on)
        app.selectDisplayQuality(item); precondition(!app.recovering)
        host.fixedPixels = nil
        app.changeDisplayQuality(to: .clarity)
        until { settled(3) }; preserved()
        precondition(host.fixedPixels == nil && host.mediaMode?.contains("PNG 无损") == true)
        app.transport.disconnect()
        until { app.recovering }
        until { settled(4) }; preserved()
        precondition(app.displayQuality == .clarity)
        try Data().write(to: root.appendingPathComponent("no-video"))
        app.changeDisplayQuality(to: .smooth)
        until { settled(5) }; preserved()
        precondition(host.mediaMode == "PNG 无损 · 视频回退")
        let sent = requests()
        let video = sent.filter { $0["type"] as? String == "video.enable" }
        precondition(video.count == 1 && video[0]["connection"] as? Int == 2 && video[0]["refine_static"] as? Bool == true)
        precondition(sent.filter { $0["type"] as? String == "session.resume" }.allSatisfy { $0["session_id"] as? String == "retained" })
        precondition(!sent.contains { ["app.launch", "window.close.request", "window.viewport"].contains($0["type"] as? String ?? "") })
        precondition(!sent.contains { $0["text"] as? String == "must not replay" })
        print("PASS: PNG default, actual lossless pixels, PNG→H.264→PNG, original NSWindows/session/related windows and pointer coordinates preserved")
        print("PASS: mode retained on reconnect, H.264 negotiation/refinement only when selected, PNG fallback, isolated shared defaults and clipboard-busy protection")
    }
}
