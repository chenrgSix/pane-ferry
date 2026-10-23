import AppKit

@main
struct AutomaticReconnectSmoke {
    static func isFixtureIcon(_ image: NSImage) -> Bool {
        // AppKit may rasterize to Retina resolution and convert the display's
        // color profile. Check the delivered blue/green translucent content,
        // rather than object identity, TIFF tags or representation dimensions.
        guard image.size == NSSize(width:96, height:64),
              let cg = image.cgImage(forProposedRect:nil, context:nil, hints:nil),
              let color = NSBitmapImageRep(cgImage:cg).colorAt(x:cg.width/2, y:cg.height/2)?.usingColorSpace(.sRGB)
        else { return false }
        return abs(color.alphaComponent - 0.5) < 0.02
            && color.redComponent < 0.5 && color.greenComponent > 0.4 && color.blueComponent > 0.6
    }

    static func verifyIcons() throws -> NSImage {
        func png(width: Int, height: Int) -> Data {
            let bitmap = NSBitmapImageRep(bitmapDataPlanes:nil, pixelsWide:width, pixelsHigh:height,
                bitsPerSample:8, samplesPerPixel:4, hasAlpha:true, isPlanar:false,
                colorSpaceName:.deviceRGB, bytesPerRow:0, bitsPerPixel:0)!
            for y in 0..<height {
                for x in 0..<width {
                    bitmap.setColor(NSColor(calibratedRed:0.2, green:0.6, blue:0.8, alpha:0.5), atX:x, y:y)
                }
            }
            return bitmap.representation(using:.png, properties:[:])!
        }
        let encoded = png(width:96, height:64).base64EncodedData()
        let icon = ApplicationIcon.decode(encoded)!
        precondition(icon.size == NSSize(width:96, height:64))
        let pixel = NSBitmapImageRep(data:icon.tiffRepresentation!)!.colorAt(x:0, y:0)!
        precondition(abs(pixel.alphaComponent - 0.5) < 0.02, "Ubuntu icon alpha survives decoding")
        precondition(ApplicationIcon.decode(Data()) == nil)
        precondition(ApplicationIcon.decode(Data("not-base64".utf8)) == nil)
        precondition(ApplicationIcon.decode(Data("<svg/>".utf8).base64EncodedData()) == nil)
        precondition(ApplicationIcon.decode(Data(repeating:65, count:24*1024+1)) == nil)
        precondition(ApplicationIcon.decode(png(width:513, height:1).base64EncodedData()) == nil)
        let pipe = Pipe()
        DispatchQueue.global().async {
            pipe.fileHandleForWriting.write(encoded.prefix(17))
            pipe.fileHandleForWriting.write(encoded.dropFirst(17))
            try? pipe.fileHandleForWriting.close()
        }
        precondition(ApplicationIcon.read(from:pipe.fileHandleForReading)?.size == icon.size)
        let args = ["--native-peer", "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", "--native-state", "/tmp/fixture", "--native-helper", "/tmp/fixture/bridge", "--session", "retained", "--icon-stdin"]
        let parsed = try Options.parse(args)
        precondition(parsed.iconFromStdin)
        do { _ = try Options.parse(args + ["--icon-stdin"]); preconditionFailure("duplicate icon flag accepted") }
        catch HostError.message { }
        return icon
    }

    static func main() throws {
        _ = NSApplication.shared
        let icon = try verifyIcons()
        let root = URL(fileURLWithPath: CommandLine.arguments[1])
        let board = NSPasteboard.withUniqueName(); defer { board.releaseGlobally() }
        let options = Options(app: nil, session: "retained", native: NativeRoute(peer: "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", state: root.path, helper: root.appendingPathComponent("fake-bridge").path))
        let app = AppDelegate(options: options, clipboard: board, applicationIcon:icon)
        defer {
            app.applicationWillTerminate(Notification(name: NSApplication.willTerminateNotification))
            for host in app.windows.values { host.closeLocally() }
        }
        var waitIndex = 0
        func until(_ condition: () -> Bool) {
            waitIndex += 1
            let deadline = Date().addingTimeInterval(8)
            while !condition(), Date() < deadline {
                RunLoop.current.run(until: Date().addingTimeInterval(0.01)); app.pump()
            }
            if !condition() {
                print("state wait=\(waitIndex) connected=\(app.connected) ready=\(app.ready) recovering=\(app.recovering) windows=\(app.windows.count) seq=\(app.lastSequence) transportAlive=\(app.transport.alive)")
                fflush(stdout)
            }
            precondition(condition(), "automatic reconnect wait \(waitIndex) did not reach expected state")
        }
        app.applicationDidFinishLaunching(Notification(name:NSApplication.didFinishLaunchingNotification))
        precondition(NSApp.mainMenu?.items.first?.title == app.applicationName)
        precondition(isFixtureIcon(NSApp.applicationIconImage),
                     "Ubuntu icon content is applied to this application's Dock tile")
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
        precondition(isFixtureIcon(NSApp.applicationIconImage),
                     "transport recovery retains this application's icon content")
        precondition(NSApp.mainMenu?.items.first?.title == app.applicationName,
                     "transport recovery retains the Ubuntu application identity")
        until { app.lastSequence >= 3 }
        app.receive(Incoming(object:["type":"input.authorization", "session_id":"other", "seq":4, "state":"pending"], image:nil))
        precondition(original.inputNotice == nil, "another session changed input status")
        app.receive(Incoming(object:["type":"input.authorization", "session_id":"retained", "seq":4, "state":"pending"], image:nil))
        precondition(original.window.subtitle.contains("Ubuntu") && original.window.subtitle.contains("授权"))
        original.mediaMode = "H.264"; original.updateSubtitle()
        precondition(original.window.subtitle.contains(app.desktopInputNotice!), "frame metadata hid the consent instructions")
        if let pixels = original.surface.image {
            precondition(original.window.subtitle.contains("接收 \(pixels.width) × \(pixels.height) 像素"), "subtitle must report decoded pixels")
        } else {
            precondition(original.window.subtitle.contains("等待画面"), "metadata alone must not claim received pixels")
        }
        app.receive(Incoming(object:["type":"error", "code":"desktop_authorization_pending"], image:nil))
        precondition(app.connected && !app.recovering, "pending permission interrupted the transport")
        app.receive(Incoming(object:["type":"input.authorization", "session_id":"retained", "seq":5, "state":"ready"], image:nil))
        precondition(original.inputNotice == nil && original.window.subtitle.contains("H.264"))
        app.receive(Incoming(object:["type":"input.authorization", "session_id":"retained", "seq":6, "state":"paused"], image:nil))
        precondition(original.window.subtitle.contains("锁屏"))
        app.receive(Incoming(object:["type":"input.authorization", "session_id":"retained", "seq":7, "state":"ready"], image:nil))
        let other = HostedWindow(id:43, width:640, height:480, revision:1, scale:1, popup:false, disconnect:{}, transmit:{ _ in })
        app.windows[43] = other
        app.receive(Incoming(object:["type":"error", "code":"resolution_limited", "window_id":42], image:nil))
        precondition(app.connected && original.resolutionNotice != nil && other.resolutionNotice == nil,
                     "a resize rejection must keep the session and other windows unaffected")
        app.windows.removeValue(forKey:43); other.closeLocally()
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

        // A lost initial launch response must reuse its request ID. Once a
        // session is known, even --app startup resumes instead of relaunching.
        let launch = AppDelegate(options:Options(app:"fixture-app", session:nil, native:options.native), clipboard:board)
        var sent:[[String:Any]]=[]; launch.wireSink = { sent.append($0) }
        let hello = Incoming(object:["type":"hello", "capabilities":["display.scale", "native.h264"]], image:nil)
        launch.receive(hello); launch.ready = false; launch.recovering = true; launch.receive(hello)
        let ids = sent.filter { $0["type"] as? String == "app.launch" }.compactMap { $0["request_id"] as? String }
        precondition(ids.count == 2 && ids[0] == ids[1])
        let expectedScale = (NSScreen.main?.backingScaleFactor ?? 1) >= 1.5 ? 2 : 1
        precondition(sent.filter { $0["type"] as? String == "app.launch" }.allSatisfy { $0["scale_factor"] as? Int == expectedScale },
                     "new launches must use the current Mac display density")
        launch.session = "known"; launch.ready = false; launch.receive(hello)
        precondition(sent.last?["type"] as? String == "session.resume" && sent.last?["session_id"] as? String == "known")
        launch.stopRecovery()
        print("PASS: real transport subprocess loss automatically resumes the same NSWindow/session, resets stream sequence, preserves resolution, drops offline input and stops on quit; launch idempotence")
        print("PASS: bounded Ubuntu PNG with alpha over fragmented stdin; non-PNG/oversized icons rejected; application Dock icon survives reconnect")
        print("PASS: shared Ubuntu authorization state stays scoped to the session, survives frame metadata, clears on readiness and does not block reconnect")
    }
}
