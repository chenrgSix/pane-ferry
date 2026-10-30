import AppKit

@main
struct ControllerInputSmoke {
    static func main() {
        _ = NSApplication.shared
        verifyShortcutRelease()
        verifyReleaseAfterShrink()
        verifyRemoteClipboardShortcuts()
        verifyCut()
        verifyLogicalViewport()
        verifyRealtimeHover()
        verifyHiddenSourceWindow()
        verifyRemoteResizeReconciliation()
        try! verifyClipboardBundles()
        let surface = SurfaceView(frame: NSRect(x: 0, y: 0, width: 640, height: 480))
        surface.remoteID = 42
        var messages: [[String: Any]] = []
        var reads = 0
        surface.transmit = { messages.append($0) }
        surface.readControllerClipboard = { reads += 1; return "控制端 clipboard 🙂" }
        surface.setMarkedText("zhong", selectedRange: NSRange(location: 5, length: 0), replacementRange: NSRange(location: NSNotFound, length: 0))
        precondition(surface.hasMarkedText() && messages.isEmpty && reads == 0, "preedit stays on controller")
        surface.insertText("中文🙂", replacementRange: NSRange(location: NSNotFound, length: 0))
        precondition(!surface.hasMarkedText() && messages.count == 1)
        precondition(messages[0]["type"] as? String == "input.text.commit" && messages[0]["text"] as? String == "中文🙂")
        let modifier = NSEvent.keyEvent(with: .flagsChanged, location: .zero, modifierFlags: [.shift], timestamp: 0,
            windowNumber: 0, context: nil, characters: "", charactersIgnoringModifiers: "", isARepeat: false, keyCode: 56)!
        surface.flagsChanged(with: modifier)
        precondition(messages.count == 1, "controller IME modifiers are not sent remotely")
        surface.paste(nil)
        precondition(reads == 1 && messages.last?["type"] as? String == "clipboard.paste")
        precondition(messages.last?["text"] as? String == "控制端 clipboard 🙂")
        surface.copy(nil)
        precondition(reads == 1 && messages.last?["type"] as? String == "clipboard.copy")
        precondition(messages.last?["request_id"] is String)
        let count = messages.count
        surface.insertText(String(repeating: "x", count: controllerTextLimit + 1), replacementRange: NSRange(location: NSNotFound, length: 0))
        precondition(messages.count == count, "oversized text is rejected, not truncated")
        var gate = ClipboardCopyGate()
        let now = Date()
        precondition(gate.accept(id: "unsolicited", window: 42, text: "bad", now: now) == nil)
        gate.begin(id: "copy", window: 42, now: now)
        precondition(gate.accept(id: "other", window: 42, text: "bad", now: now) == nil)
        precondition(gate.accept(id: "copy", window: 7, text: "bad", now: now) == nil)
        precondition(gate.accept(id: "copy", window: 42, text: "复制🙂", now: now) == "复制🙂")
        precondition(gate.accept(id: "copy", window: 42, text: "replay", now: now) == nil)
        gate.begin(id: "expired", window: 42, now: now)
        precondition(gate.accept(id: "expired", window: 42, text: "late", now: now.addingTimeInterval(6)) == nil)
        let retina = RenderGeometry(scale: 2)!
        let size = retina.points(width: 2000, height: 1400)
        precondition(size == CGSize(width: 1000, height: 700))
        let pixels = retina.pixels(forLogicalSize: size)!
        precondition(pixels.width == 2000 && pixels.height == 1400, "Retina uses physical pixels without changing point size")
        let maximum = retina.pixels(forLogicalSize: CGSize(width: 10_000, height: 10_000))!
        precondition(maximum.width <= 4096 && maximum.height <= 2160 && maximum.width * maximum.height <= 8_000_000)
        precondition(retina.pixels(forLogicalSize: CGSize(width: CGFloat.infinity, height: 100)) == nil)
        let screenPixels = RenderGeometry(scale: 1)!.pixels(forLogicalSize: CGSize(width: 1000, height: 700))!
        precondition(screenPixels.width == 1000 && screenPixels.height == 700, "automatic size follows logical points at the existing source density")
        let wide = retina.pixels(forLogicalSize: CGSize(width: 3000, height: 1000))!
        precondition(abs(Double(wide.width) / Double(wide.height) - 3) < 0.003, "frame limits preserve the local window aspect ratio")
        precondition(RenderGeometry(scale: 3) == nil)
        let fit = RenderGeometry.imageRect(in: CGRect(x: 0, y: 0, width: 1200, height: 800), width: 1920, height: 720)
        precondition(fit == CGRect(x: 0, y: 175, width: 1200, height: 450), "different aspect ratios must be fitted without stretching")
        surface.remoteWidth = 1920; surface.remoteHeight = 720
        let beforeAutomaticClick = messages.count
        let automaticClick = NSEvent.mouseEvent(with: .leftMouseDown, location: CGPoint(x: 10, y: 470), modifierFlags: [], timestamp: 0, windowNumber: 0, context: nil, eventNumber: 1, clickCount: 1, pressure: 1)!
        surface.mouseDown(with: automaticClick)
        precondition(messages.count == beforeAutomaticClick, "automatic mode also rejects clicks in letterbox bars")
        surface.releaseInput()
        let beforePointer = messages.count
        let outside = NSEvent.mouseEvent(with: .leftMouseDown, location: CGPoint(x: 0, y: 0), modifierFlags: [], timestamp: 0, windowNumber: 0, context: nil, eventNumber: 1, clickCount: 1, pressure: 1)!
        surface.mouseDown(with: outside)
        precondition(messages.count == beforePointer, "clicks in letterbox bars must not reach the application")
        let center = NSEvent.mouseEvent(with: .leftMouseDown, location: CGPoint(x: 320, y: 240), modifierFlags: [], timestamp: 0, windowNumber: 0, context: nil, eventNumber: 2, clickCount: 1, pressure: 1)!
        surface.mouseDown(with: center)
        precondition(messages.last?["x"] as? Int == 960 && messages.last?["y"] as? Int == 360, "pointer maps through the fitted image rectangle")

        var resizeMessages: [[String: Any]] = []
        var disconnects = 0
        let host = HostedWindow(id: 7, width: 1280, height: 800, revision: 1, scale: 1, popup: false, disconnect: { disconnects += 1 }) { resizeMessages.append($0) }
        host.remoteVisible = true
        precondition(host.fixedPixels == nil, "new windows request automatic resolution")
        host.window.setContentSize(NSSize(width: 900, height: 600))
        host.windowDidResize(Notification(name: NSWindow.didResizeNotification))
        let viewport = host.surface.bounds.size
        host.applyRemoteGeometry(width: 1280, height: 800, rendering: RenderGeometry(scale: 1)!)
        precondition(host.surface.bounds.size == viewport, "late remote geometry cannot undo a local resize")
        // AppKit can also emit focus/input-release notifications here. Only
        // the configure message proves that the common-mode timer fired.
        let deadline = Date().addingTimeInterval(1)
        while !resizeMessages.contains(where: { $0["type"] as? String == "window.configure" }) && Date() < deadline {
            RunLoop.current.run(mode: .eventTracking, before: deadline)
        }
        let expected = RenderGeometry(scale: 1)!.pixels(forLogicalSize: viewport)!
        let duringTracking = resizeMessages.last { $0["type"] as? String == "window.configure" }
        precondition(duringTracking?["width"] as? Int == expected.width && duringTracking?["height"] as? Int == expected.height,
                     "resolution updates run during AppKit event tracking, without waiting for the drag to end: \(resizeMessages), viewport=\(viewport)")
        host.window.setContentSize(NSSize(width: 700, height: 500))
        host.windowDidEndLiveResize(Notification(name: NSWindow.didEndLiveResizeNotification))
        let smaller = RenderGeometry(scale: 1)!.pixels(forLogicalSize: host.surface.bounds.size)!
        precondition(resizeMessages.last?["width"] as? Int == smaller.width && resizeMessages.last?["height"] as? Int == smaller.height)
        host.fixedPixels = (2560, 1664)
        host.windowDidEndLiveResize(Notification(name: NSWindow.didEndLiveResizeNotification))
        precondition(resizeMessages.last?["width"] as? Int == 2560)
        host.useAutomaticResolution()
        host.windowDidEndLiveResize(Notification(name: NSWindow.didEndLiveResizeNotification))
        precondition(resizeMessages.last?["width"] as? Int == smaller.width,
                     "switching back to automatic resolution immediately follows the current viewport")
        let context = CGContext(data: nil, width: 1920, height: 720, bitsPerComponent: 8,
                                bytesPerRow: 1920 * 4, space: CGColorSpaceCreateDeviceRGB(),
                                bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue)!
        let oldFrame = context.makeImage()!
        host.present(oldFrame, mediaMode: "H.264 视频")
        host.showActualPixels()
        precondition(abs(host.surface.bounds.width * host.window.backingScaleFactor - 1920) < 1)
        precondition(abs(host.surface.bounds.height * host.window.backingScaleFactor - 720) < 1)
        precondition(host.fixedPixels?.width == 1920 && host.fixedPixels?.height == 720,
                     "actual-pixel display must neither resample the source nor change application scale")
        surface.image = oldFrame
        surface.remoteWidth = 1200; surface.remoteHeight = 800
        precondition(surface.imageRect.height == 240, "late old frame keeps its own aspect ratio during resize")
        surface.releaseInput()
        let beforeStale = messages.count
        surface.mouseDown(with: center)
        precondition(messages.count == beforeStale, "stale frame cannot click a different remote layout")
        host.scaleMode = "source"
        host.present(oldFrame, mediaMode: "H.264 视频")
        precondition(host.window.subtitle.contains("接收 1920 × 720") && host.window.subtitle.contains("沿用应用缩放"))
        precondition(!host.window.subtitle.contains("100%"), "existing process scale is not measured by the protocol default")
        precondition(host.window.subtitle.contains("目标"), "a resize request must not be reported as received pixels")
        let beforeClose = resizeMessages.count
        host.window.performClose(nil)
        host.window.performClose(nil)
        precondition(disconnects == 1 && resizeMessages.count == beforeClose,
            "the controller close button must detach once without closing the remote application")
        host.closeLocally()
        precondition(disconnects == 1, "remote window destruction only closes the local mirror")
        let child = HostedWindow(id: 8, width: 480, height: 320, revision: 1, scale: 1, popup: false, disconnect: { disconnects += 1 }) { resizeMessages.append($0) }
        child.relatedWindow = true
        let beforeChildClose = resizeMessages.count
        child.window.performClose(nil)
        precondition(disconnects == 2 && resizeMessages.count == beforeChildClose,
            "closing a local related window must preserve its remote application")
        let childCloseCount = resizeMessages.count
        child.closeLocally()
        precondition(disconnects == 2 && resizeMessages.count == childCloseCount, "server destruction must not echo a child close")
        print("PASS: controller-only preedit/commit, on-demand paste/copy, input bounds, correlated clipboard replies without touching the system clipboard")
        print("PASS: Retina point/pixel mapping, resize round trip and bounded render dimensions")
        print("PASS: automatic viewport resolution, live resize tracking, stale geometry isolation and manual/automatic switching")
        print("PASS: controller close detaches once; remote destruction stays local")
    }

    private static func verifyRemoteResizeReconciliation() {
        var sent: [[String: Any]] = []
        let host = HostedWindow(id: 93, width: 900, height: 600, revision: 1, scale: 1,
                                popup: false, disconnect: {}) { sent.append($0) }
        defer { host.closeLocally() }
        host.remoteVisible = true
        host.windowDidEndLiveResize(Notification(name: NSWindow.didEndLiveResizeNotification))
        let target = host.requestedPixels!
        let viewport = host.surface.bounds.size
        let scale = RenderGeometry(scale: 1)!
        func wait(_ seconds: Double) { RunLoop.current.run(until: Date().addingTimeInterval(seconds)) }
        func geometry(_ width: Int, _ height: Int) { host.applyRemoteGeometry(width: width, height: height, rendering: scale) }
        func requests() -> [[String: Any]] { sent.filter { $0["type"] as? String == "window.configure" } }
        func waitForRequests(_ count: Int) {
            let deadline = Date().addingTimeInterval(2)
            while requests().count < count && Date() < deadline {
                RunLoop.current.run(until: Date().addingTimeInterval(0.005))
            }
            precondition(requests().count == count,
                         "resize requests=\(requests()) expected=\(count) target=\(target) viewport=\(host.surface.bounds.size)")
        }

        // AppKit may constrain the initial window to the runner's display.
        // Acknowledge that initial viewport before testing a source resize.
        geometry(target.width, target.height)
        sent.removeAll()

        // A source maximize after an already matched viewport must recover
        // without requiring another local drag or a menu action.
        geometry(3072, 1504)
        waitForRequests(1)
        precondition(requests().count == 1 && requests()[0]["width"] as? Int == target.width &&
                     requests()[0]["height"] as? Int == target.height)
        geometry(target.width, target.height)
        wait(0.45)
        precondition(requests().count == 1 && host.surface.bounds.size == viewport)

        // Transient geometry that settles by itself must not cause a resize.
        geometry(3072, 1504); geometry(target.width, target.height)
        wait(0.45)
        precondition(requests().count == 1)
        geometry(target.width, target.height + 1)
        wait(0.45)
        precondition(requests().count == 1, "one pixel of compositor rounding must not cause repeated requests")

        // A constrained app or lost reply cannot produce a feedback loop.
        geometry(1000, 700)
        waitForRequests(3)
        precondition(requests().count == 3)
        for _ in 0..<10 { geometry(1000, 700) }
        wait(0.45)
        precondition(requests().count == 3)

        geometry(target.width, target.height)
        geometry(3072, 1504)
        host.applyRemoteVisibility(false)
        wait(0.45)
        precondition(requests().count == 3, "hidden source cancels pending resize")
        host.applyRemoteVisibility(true)
        waitForRequests(4)
        precondition(requests().count == 4, "restoration reconciles the original viewport")
        geometry(target.width, target.height)
        for mode in ["related", "desktop", "session"] {
            host.relatedWindow = mode == "related"; host.desktopMode = mode == "desktop"; host.sessionMode = mode == "session"
            geometry(3072, 1504); wait(0.45)
            precondition(requests().count == 4, "source-sized children and fixed canvases are not resized")
            geometry(target.width, target.height)
        }
        host.relatedWindow = false; host.desktopMode = false; host.sessionMode = false
        geometry(3072, 1504); host.closeLocally(); wait(0.45)
        precondition(requests().count == 4, "closing cancels reconciliation")
        print("PASS: source resize reconciliation, transient suppression, bounded retries, visibility and fixed-canvas isolation")
    }

    private static func verifyReleaseAfterShrink() {
        let surface = SurfaceView(frame: NSRect(x: 0, y: 0, width: 640, height: 480))
        surface.remoteID = 77; surface.remoteWidth = 640; surface.remoteHeight = 480
        var sent: [[String: Any]] = []
        surface.transmit = { sent.append($0) }
        let down = NSEvent.mouseEvent(with: .leftMouseDown, location: CGPoint(x: 320, y: 240), modifierFlags: [], timestamp: 0,
            windowNumber: 0, context: nil, eventNumber: 1, clickCount: 1, pressure: 1)!
        surface.mouseDown(with: down)
        precondition(sent.last?["pressed"] as? Bool == true)
        surface.remoteWidth = 100; surface.remoteHeight = 80
        surface.releaseInput()
        precondition(sent.last?["pressed"] as? Bool == false)
        precondition(sent.last?["x"] as? Int == 99 && sent.last?["y"] as? Int == 79,
                     "focus loss after shrink must release the held button inside current remote bounds")
        let count = sent.count
        surface.releaseInput()
        precondition(sent.count == count, "repeated cleanup cannot replay releases")
    }

    private static func verifyRemoteClipboardShortcuts() {
        let surface = SurfaceView(frame: .zero)
        surface.remoteID = 1; surface.remoteClipboardShortcuts = true
        var sent: [[String: Any]] = []
        surface.transmit = { sent.append($0) }
        surface.readControllerClipboard = { preconditionFailure("remote-only shortcuts must not read the local clipboard") }
        surface.requestControllerPaste = { preconditionFailure("remote-only paste must not upload the local clipboard") }
        surface.copy(nil); surface.cut(nil); surface.paste(nil)
        precondition(sent.count == 12 && sent.allSatisfy { $0["type"] as? String == "input.key" })
        for (index, symbol) in [UInt32(99), 120, 118].enumerated() {
            let keys = Array(sent[index*4..<(index+1)*4])
            precondition(keys.map { $0["keysym"] as! UInt32 } == [0xffe3, symbol, symbol, 0xffe3])
            precondition(keys.map { $0["pressed"] as! Bool } == [true, true, false, false])
        }
        print("PASS: Windows copy/cut/paste shortcuts stay in the remote clipboard and release modifiers")
    }

    private static func verifyHiddenSourceWindow() {
        var sent: [[String: Any]] = []
        let host = HostedWindow(id: 92, width: 640, height: 480, revision: 1, scale: 1,
                                popup: false, disconnect: {}) { sent.append($0) }
        defer { host.closeLocally() }
        host.applyRemoteVisibility(true)
        let frame = host.window.frame
        host.surface.insertText("before hide", replacementRange: NSRange(location: NSNotFound, length: 0))
        host.applyRemoteVisibility(false)
        precondition(host.window.isVisible && host.window.frame == frame && !host.hiddenSourceView.isHidden,
                     "a hidden source keeps a recoverable local window with the same geometry")
        precondition(host.surface.transmit == nil && host.window.subtitle.contains("可恢复"))
        let count = sent.count
        host.surface.insertText("hidden input", replacementRange: NSRange(location: NSNotFound, length: 0))
        host.windowDidBecomeKey(Notification(name: NSWindow.didBecomeKeyNotification))
        host.windowDidEndLiveResize(Notification(name: NSWindow.didEndLiveResizeNotification))
        precondition(sent.count == count, "hidden content must not receive typing, focus or resize")
        host.restoreSourceButton.performClick(nil)
        precondition(sent.count == count + 1 && sent.last?["type"] as? String == "window.focus"
                     && sent.last?["window_id"] as? UInt32 == 92)
        precondition(!host.remoteVisible && !host.hiddenSourceView.isHidden && host.surface.transmit == nil,
                     "restore waits for confirmed source visibility")
        host.applyRemoteVisibility(true)
        precondition(host.hiddenSourceView.isHidden && host.surface.transmit != nil && host.window.frame == frame)
        let beforeMinimize = sent.count
        host.window.miniaturize(nil)
        let deadline = Date().addingTimeInterval(2)
        while !host.window.isMiniaturized && Date() < deadline {
            RunLoop.current.run(until: Date().addingTimeInterval(0.01))
        }
        precondition(host.window.isMiniaturized, "AppKit must complete the local minimize first")
        precondition(sent.count == beforeMinimize, "native minimize must not minimize the source")
        host.applyRemoteVisibility(false); host.applyRemoteVisibility(true)
        precondition(host.window.isMiniaturized, "source visibility must not undo local minimize")
        host.window.deminiaturize(nil)
        let child = HostedWindow(id: 93, width: 300, height: 200, revision: 1, scale: 1,
                                 popup: false, disconnect: {}) { sent.append($0) }
        defer { child.closeLocally() }
        child.relatedWindow = true; child.applyRemoteVisibility(true); child.applyRemoteVisibility(false)
        precondition(!child.window.isVisible && child.hiddenSourceView.isHidden,
                     "dismissed child windows must not become restore placeholders")
        let restored = HostedWindow(id: 94, width: 640, height: 480, revision: 1, scale: 1,
                                    popup: false, disconnect: {}) { sent.append($0) }
        defer { restored.closeLocally() }
        restored.applyRemoteVisibility(false)
        precondition(restored.window.isVisible && !restored.hiddenSourceView.isHidden,
                     "resuming an already minimized session must expose its restore action")
        print("PASS: source hide has a restore action, blocks stale input, preserves local minimize and handles hidden snapshots")
    }

    private static func verifyRealtimeHover() {
        let surface = SurfaceView(frame: NSRect(x: 0, y: 0, width: 1000, height: 500))
        surface.remoteID = 91
        surface.remoteWidth = 1000; surface.remoteHeight = 500
        var sent: [[String: Any]] = []
        surface.transmit = { sent.append($0) }
        func event(_ type: NSEvent.EventType, _ x: CGFloat) -> NSEvent {
            NSEvent.mouseEvent(with: type, location: surface.convert(CGPoint(x: x, y: 250), to: nil),
                               modifierFlags: [], timestamp: 0, windowNumber: 0, context: nil,
                               eventNumber: 1, clickCount: 1, pressure: 1)!
        }
        surface.mouseMoved(with: event(.mouseMoved, 100))
        precondition(sent.count == 1 && sent[0]["x"] as? Int == 100, "hover must reach the remote window immediately")
        surface.mouseMoved(with: event(.mouseMoved, 200))
        precondition(sent.count == 2 && sent[1]["x"] as? Int == 200 && sent[1]["button"] as? Int == 0,
                     "subsequent hover uses the latest mapped position")

        surface.mouseMoved(with: event(.mouseMoved, 300))
        surface.mouseDown(with: event(.leftMouseDown, 400))
        surface.mouseDragged(with: event(.leftMouseDragged, 500))
        surface.mouseUp(with: event(.leftMouseUp, 600))
        precondition(sent.count == 6 && sent[2]["x"] as? Int == 300
                     && sent[3]["x"] as? Int == 400 && sent[3]["pressed"] as? Bool == true
                     && sent[4]["x"] as? Int == 500 && sent[4]["button"] as? Int == 0
                     && sent[5]["x"] as? Int == 600 && sent[5]["pressed"] as? Bool == false,
                     "hover, click and drag preserve their immediate order")

        surface.mouseMoved(with: event(.mouseMoved, 700))
        surface.mouseExited(with: event(.mouseMoved, 700))
        precondition(sent.count == 7 && sent[6]["x"] as? Int == 700, "leaving does not delay the last hover")
        surface.mouseMoved(with: event(.mouseMoved, 800))
        surface.releaseInput()
        precondition(sent.count == 8 && sent[7]["x"] as? Int == 800, "release does not replay an old hover")
        print("PASS: hover, clicks and drags transmit immediately in order without a 200 ms delay")
    }

    private static func verifyLogicalViewport() {
        // Exercise the production AppKit path at both source densities. The
        // same rule covers native Wayland, Xwayland/X11 and a Mac publisher.
        for scale in [1, 2] {
            var sent: [[String: Any]] = []
            let host = HostedWindow(id: 77, width: 640 * scale, height: 480 * scale,
                                    revision: 1, scale: scale, popup: false, disconnect: {}) { sent.append($0) }
            defer { host.closeLocally() }
            host.remoteVisible = true
            host.window.setContentSize(CGSize(width: 1470, height: 802))
            host.windowDidEndLiveResize(Notification(name: NSWindow.didEndLiveResizeNotification))
            precondition(sent.last?["width"] as? Int == 1470 * scale && sent.last?["height"] as? Int == 802 * scale,
                         "logical size must be identical at source 1x and 2x; do not impose Mac backing density")
            precondition(!sent.contains { $0["type"] as? String == "window.viewport" || $0["scale_factor"] != nil },
                         "resizing must not request a DPI change")
            func frame(_ width: Int, _ height: Int) -> CGImage {
                CGContext(data: nil, width: width, height: height, bitsPerComponent: 8,
                          bytesPerRow: width * 4, space: CGColorSpaceCreateDeviceRGB(),
                          bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue)!.makeImage()!
            }
            host.applyRemoteGeometry(width: 1470 * scale, height: 802 * scale, rendering: RenderGeometry(scale: scale)!)
            host.present(frame(1470 * scale, 802 * scale), mediaMode: "PNG")
            precondition(!host.window.subtitle.contains("已达像素预算"), "1x on Retina is intentional, not budget reduction")
            precondition(host.window.subtitle.contains("本机 1470 × 802 点"))
            // Resize the viewport before the remote repaint: the old image
            // must keep its ratio and the letterbox offset must enter mapping.
            host.window.setContentSize(CGSize(width: 900, height: 900))
            let surface = host.surface, rect = surface.imageRect
            precondition(abs(rect.width / rect.height - 1470.0 / 802) < 0.0001 && rect.minY > 0)
            func event(_ type: NSEvent.EventType, _ point: CGPoint) -> NSEvent {
                NSEvent.mouseEvent(with: type, location: surface.convert(point, to: nil), modifierFlags: [],
                                  timestamp: 0, windowNumber: host.window.windowNumber, context: nil,
                                  eventNumber: 1, clickCount: 1, pressure: 1)!
            }
            for fraction in [0.001, 0.25, 0.5, 0.999] {
                let point = CGPoint(x: rect.minX + rect.width * fraction, y: rect.minY + rect.height * fraction)
                surface.mouseDown(with: event(.leftMouseDown, point))
                precondition(abs((sent.last?["x"] as! Int) - Int(Double(1470 * scale) * fraction)) <= 1)
                precondition(abs((sent.last?["y"] as! Int) - Int(Double(802 * scale) * fraction)) <= 1)
                surface.mouseUp(with: event(.leftMouseUp, point))
            }
            let beforeBar = sent.count
            surface.mouseDown(with: event(.leftMouseDown, CGPoint(x: 20, y: 1)))
            precondition(sent.count == beforeBar, "letterbox bars do not reach remote windows")
            // A drag that leaves the image must release at the image edge.
            surface.mouseDown(with: event(.leftMouseDown, CGPoint(x: rect.midX, y: rect.midY)))
            surface.mouseUp(with: event(.leftMouseUp, CGPoint(x: 1000, y: 1000)))
            precondition(sent.last?["pressed"] as? Bool == false && sent.last?["x"] as? Int == 1470 * scale - 1
                         && sent.last?["y"] as? Int == 802 * scale - 1)
            // Metadata arriving before pixels must not send clicks into a
            // different layout, but must release already-held mouse buttons.
            surface.mouseDown(with: event(.leftMouseDown, CGPoint(x: rect.midX, y: rect.midY)))
            host.applyRemoteGeometry(width: 600 * scale, height: 400 * scale, rendering: RenderGeometry(scale: scale)!)
            surface.mouseUp(with: event(.leftMouseUp, CGPoint(x: rect.midX, y: rect.midY)))
            precondition(sent.last?["pressed"] as? Bool == false && (sent.last?["x"] as! Int) < 600 * scale
                         && (sent.last?["y"] as! Int) < 400 * scale)
            let beforeStale = sent.count
            surface.mouseDown(with: event(.leftMouseDown, CGPoint(x: rect.midX, y: rect.midY)))
            precondition(sent.count == beforeStale)
            host.windowDidChangeBackingProperties(Notification(name: NSWindow.didChangeBackingPropertiesNotification))
            host.windowDidEndLiveResize(Notification(name: NSWindow.didEndLiveResizeNotification))
            precondition(sent.last?["width"] as? Int == 900 * scale && sent.last?["height"] as? Int == 900 * scale,
                         "moving between Mac backing densities does not alter remote UI scale")
            // The user's 900 × 700-point example must request source pixels
            // at the existing scale and keep pointer mapping on the new frame.
            host.window.setContentSize(CGSize(width: 900, height: 700))
            host.windowDidEndLiveResize(Notification(name: NSWindow.didEndLiveResizeNotification))
            precondition(sent.last?["width"] as? Int == 900 * scale && sent.last?["height"] as? Int == 700 * scale)
            host.applyRemoteGeometry(width: 900 * scale, height: 700 * scale, rendering: RenderGeometry(scale: scale)!)
            host.present(frame(900 * scale, 700 * scale), mediaMode: "H.264")
            let center = CGPoint(x: host.surface.imageRect.midX, y: host.surface.imageRect.midY)
            surface.mouseDown(with: event(.leftMouseDown, center))
            precondition(sent.last?["x"] as? Int == 450 * scale && sent.last?["y"] as? Int == 350 * scale)
            surface.mouseUp(with: event(.leftMouseUp, center))
        }
        print("PASS: logical 1470x802 and 900x700 at source 1x/2x, equal aspect ratios, corner/center input, letterbox rejection and drag release during repaint")
    }

    private static func verifyShortcutRelease() {
        let surface = SurfaceView(frame: .zero)
        surface.remoteID = 42
        var held = Set<UInt32>()
        var presses: [UInt32] = []
        var messages: [[String: Any]] = []
        surface.transmit = { message in
            messages.append(message)
            guard message["type"] as? String == "input.key", let symbol = message["keysym"] as? UInt32 else { return }
            if message["pressed"] as? Bool == true {
                if symbol == 0x61 { precondition(held.contains(0xffe3), "select-all never becomes an unmodified A") }
                held.insert(symbol); presses.append(symbol)
            } else {
                if symbol == 0xffe3 { precondition(!held.contains(0x61), "A is released before Ctrl to prevent unmodified autorepeat") }
                held.remove(symbol)
            }
        }
        func event(_ type: NSEvent.EventType, _ code: UInt16, _ flags: NSEvent.ModifierFlags, repeat repeating: Bool = false) -> NSEvent {
            NSEvent.keyEvent(with: type, location: .zero, modifierFlags: flags, timestamp: 0,
                windowNumber: 0, context: nil, characters: "", charactersIgnoringModifiers: "", isARepeat: repeating, keyCode: code)!
        }
        // AppKit can omit the letter's keyUp for a Command shortcut. The remote
        // X server must never retain a key that can autorepeat after Ctrl drops.
        surface.keyDown(with: event(.keyDown, 0, [.command]))
        precondition(presses.contains(0x61) && held.isEmpty, "Command+A completes without depending on A keyUp")
        surface.keyDown(with: event(.keyDown, 0, [.command], repeat: true))
        precondition(presses.filter { $0 == 0x61 }.count == 2 && held.isEmpty, "controller repeat produces one complete shortcut per event")
        surface.keyDown(with: event(.keyDown, 7, [.command]))
        surface.flagsChanged(with: event(.flagsChanged, 55, []))
        precondition(held.isEmpty && !messages.contains { $0["type"] as? String == "input.text.commit" },
                     "select-all followed by unsupported cut cannot leave A repeating")
        let beforeOrphanRelease = messages.count
        surface.keyUp(with: event(.keyUp, 0, []))
        precondition(messages.count == beforeOrphanRelease, "a delayed Command keyUp does not emit unmatched releases")
        surface.keyDown(with: event(.keyDown, 123, [.command, .shift]))
        precondition(presses.contains(0xff51) && held.isEmpty, "multi-modifier Command shortcuts release the navigation key and all modifiers")
        surface.keyDown(with: event(.keyDown, 123, [.control]))
        precondition(held == [0xffe3, 0xff51], "Control navigation retains its physical down/up lifecycle")
        surface.keyUp(with: event(.keyUp, 123, [.control]))
        precondition(held.isEmpty, "physical keyUp releases Control navigation")
        surface.keyDown(with: event(.keyDown, 123, [.control]))
        surface.releaseInput()
        precondition(held.isEmpty, "focus loss still releases physical keys")
        surface.keyDown(with: event(.keyDown, 0, [.control]))
        surface.keyDown(with: event(.keyDown, 7, [.command]))
        precondition(held.isEmpty, "an intercepted Command shortcut releases any earlier physical chord")
        print("PASS: Command shortcuts, repeat and select-all/cut cannot strand remote keys when AppKit omits keyUp")
    }

    private static func verifyCut() {
        let surface = SurfaceView(frame: .zero)
        surface.remoteID = 42
        var messages: [[String: Any]] = []
        surface.transmit = { messages.append($0) }
        surface.readControllerClipboard = { preconditionFailure("cut never reads the controller clipboard") }
        surface.cut(nil)
        precondition(messages.isEmpty, "old agents without clipboard.cut never receive a raw destructive fallback")
        surface.controllerCutEnabled = true
        surface.setMarkedText("zhong", selectedRange: NSRange(location: 5, length: 0), replacementRange: NSRange(location: NSNotFound, length: 0))
        let down = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [.command], timestamp: 0,
            windowNumber: 0, context: nil, characters: "x", charactersIgnoringModifiers: "x", isARepeat: false, keyCode: 7)!
        surface.keyDown(with: down)
        precondition(!surface.hasMarkedText() && messages.count == 1)
        precondition(messages[0]["type"] as? String == "clipboard.cut" && messages[0]["window_id"] as? UInt32 == 42)
        let first = messages[0]["request_id"] as! String
        let repeated = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [.command], timestamp: 0,
            windowNumber: 0, context: nil, characters: "x", charactersIgnoringModifiers: "x", isARepeat: true, keyCode: 7)!
        surface.keyDown(with: repeated)
        precondition(messages.count == 1, "holding Command+X cannot queue repeated destructive clipboard requests")
        surface.cut(nil)
        precondition(messages.count == 2 && messages[1]["request_id"] as? String != first,
                     "the edit menu and Command+X use explicit, uniquely correlated cut requests")
        var deferredPastes = 0
        surface.requestControllerPaste = { deferredPastes += 1 }
        surface.paste(nil)
        precondition(deferredPastes == 1 && messages.count == 2, "paste intent can wait for copy/cut without reading stale clipboard contents")
        print("PASS: Command+X/menu cut uses negotiated clipboard.cut without raw key fallback or controller clipboard reads")
    }
}
