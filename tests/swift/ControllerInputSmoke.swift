import AppKit

@main
struct ControllerInputSmoke {
    static func main() {
        _ = NSApplication.shared
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
        let automaticClick = NSEvent.mouseEvent(with: .leftMouseDown, location: CGPoint(x: 10, y: 470), modifierFlags: [], timestamp: 0, windowNumber: 0, context: nil, eventNumber: 1, clickCount: 1, pressure: 1)!
        surface.mouseDown(with: automaticClick)
        precondition(messages.last?["x"] as? Int == 30 && messages.last?["y"] as? Int == 15, "automatic mode fills the viewport and maps input through the full image")
        surface.releaseInput()
        surface.preservesAspectRatio = true
        let beforePointer = messages.count
        let outside = NSEvent.mouseEvent(with: .leftMouseDown, location: CGPoint(x: 0, y: 0), modifierFlags: [], timestamp: 0, windowNumber: 0, context: nil, eventNumber: 1, clickCount: 1, pressure: 1)!
        surface.mouseDown(with: outside)
        precondition(messages.count == beforePointer, "clicks in letterbox bars must not reach the application")
        let center = NSEvent.mouseEvent(with: .leftMouseDown, location: CGPoint(x: 320, y: 240), modifierFlags: [], timestamp: 0, windowNumber: 0, context: nil, eventNumber: 2, clickCount: 1, pressure: 1)!
        surface.mouseDown(with: center)
        precondition(messages.last?["x"] as? Int == 960 && messages.last?["y"] as? Int == 360, "pointer maps through the fitted image rectangle")

        var resizeMessages: [[String: Any]] = []
        let host = HostedWindow(id: 7, width: 1280, height: 800, revision: 1, scale: 1, popup: false) { resizeMessages.append($0) }
        host.remoteVisible = true
        precondition(host.fixedPixels == nil && !host.surface.preservesAspectRatio, "new windows default to automatic resolution without letterboxing")
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
        precondition(host.surface.preservesAspectRatio && resizeMessages.last?["width"] as? Int == 2560)
        host.useAutomaticResolution()
        host.windowDidEndLiveResize(Notification(name: NSWindow.didEndLiveResizeNotification))
        precondition(!host.surface.preservesAspectRatio && resizeMessages.last?["width"] as? Int == smaller.width,
                     "switching back to automatic resolution immediately follows the current viewport")
        host.closeLocally()
        print("PASS: controller-only preedit/commit, on-demand paste/copy, input bounds, correlated clipboard replies without touching the system clipboard")
        print("PASS: Retina point/pixel mapping, resize round trip and bounded render dimensions")
        print("PASS: automatic viewport resolution, live resize tracking, stale geometry isolation and manual/automatic switching")
    }
}
