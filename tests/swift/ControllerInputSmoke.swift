import AppKit

@main
struct ControllerInputSmoke {
    static func main() {
        _ = NSApplication.shared
        verifyShortcutRelease()
        verifyCut()
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
        let pixels = retina.pixels(for: size)!
        precondition(pixels.width == 2000 && pixels.height == 1400, "Retina uses physical pixels without changing point size")
        let maximum = retina.pixels(for: CGSize(width: 10_000, height: 10_000))!
        precondition(maximum.width <= 4096 && maximum.height <= 2160 && maximum.width * maximum.height <= 8_000_000)
        precondition(retina.pixels(for: CGSize(width: CGFloat.infinity, height: 100)) == nil)
        let screenPixels = RenderGeometry(scale: 1)!.pixels(for: CGSize(width: 1000, height: 700), backingScale: 2)!
        precondition(screenPixels.width == 2000 && screenPixels.height == 1400, "automatic resolution follows the local screen density independently of UI scale")
        let wide = retina.pixels(for: CGSize(width: 3000, height: 1000))!
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
        let deadline = Date().addingTimeInterval(0.2)
        while resizeMessages.isEmpty && Date() < deadline {
            RunLoop.current.run(mode: .eventTracking, before: deadline)
        }
        let expected = retina.pixels(for: viewport, backingScale: host.window.backingScaleFactor)!
        precondition(resizeMessages.last?["width"] as? Int == expected.width && resizeMessages.last?["height"] as? Int == expected.height,
                     "resolution updates run during AppKit event tracking, without waiting for the drag to end")
        host.window.setContentSize(NSSize(width: 700, height: 500))
        host.windowDidEndLiveResize(Notification(name: NSWindow.didEndLiveResizeNotification))
        let smaller = retina.pixels(for: host.surface.bounds.size, backingScale: host.window.backingScaleFactor)!
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
        host.present(oldFrame, mediaMode: "PNG 无损刷新")
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
        host.present(oldFrame, mediaMode: "PNG 图片")
        precondition(host.window.subtitle.contains("接收 1920 × 720") && host.window.subtitle.contains("沿用应用缩放"))
        precondition(!host.window.subtitle.contains("100%"), "existing process scale is not measured by the protocol default")
        precondition(host.window.subtitle.contains("目标"), "a resize request must not be reported as received pixels")
        host.window.performClose(nil)
        precondition(disconnects == 1 && !resizeMessages.contains { $0["type"] as? String == "window.close.request" },
                     "the controller close button disconnects without closing the remote application")
        host.window.performClose(nil)
        precondition(disconnects == 1, "repeated close does not start a second detach")
        host.closeLocally()
        precondition(disconnects == 1, "remote window destruction only closes the local mirror")
        let child = HostedWindow(id: 8, width: 480, height: 320, revision: 1, scale: 1, popup: false, disconnect: { disconnects += 1 }) { resizeMessages.append($0) }
        child.relatedWindow = true
        child.window.performClose(nil)
        precondition(disconnects == 1 && resizeMessages.last?["type"] as? String == "window.close.request"
            && resizeMessages.last?["window_id"] as? UInt32 == 8, "closing a preview requests only its remote window close")
        let childCloseCount = resizeMessages.count
        child.closeLocally()
        precondition(disconnects == 1 && resizeMessages.count == childCloseCount, "server destruction must not echo a child close")
        print("PASS: controller-only preedit/commit, on-demand paste/copy, input bounds, correlated clipboard replies without touching the system clipboard")
        print("PASS: Retina point/pixel mapping, resize round trip and bounded render dimensions")
        print("PASS: automatic viewport resolution, live resize tracking, stale geometry isolation and manual/automatic switching")
        print("PASS: controller close disconnects once without a remote close request; remote destruction stays local")
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
