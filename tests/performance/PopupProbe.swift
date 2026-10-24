// Read-only application exercise: open a context menu at an observed location,
// then dismiss that test-owned menu with Escape. No production source changes.
import AppKit

enum Probe {
    static let lock = NSLock()
    static var rows: [[String: Any]] = []
    static var current: [String: Any]?
    static var frames: [ObjectIdentifier: [String: Any]] = [:]
    static var delegate: AppDelegate?
    static var injecting = false
    static var phase = "waiting"
    static var rootID: UInt32?
    static var openedID: UInt32?
    static var samples = 30
    static var count = 0
    static var point = CGPoint.zero
    static var phaseStarted = now()
    static var preview = false
    static var serial = -1
    static var finishing = false
    static var tick: Timer?
    static let output = ProcessInfo.processInfo.environment["PROBE_OUTPUT"]!
    static let command = ProcessInfo.processInfo.environment["PROBE_COMMAND"]!
    static func now() -> UInt64 { DispatchTime.now().uptimeNanoseconds }

    static func configure(_ host: HostedWindow) {
        // Preserve the application's original source geometry, including DPI.
        host.fixedPixels = (host.surface.remoteWidth, host.surface.remoteHeight)
        let available = (host.window.screen ?? NSScreen.main!).visibleFrame.size
        let ratio = min(1, min(available.width / CGFloat(host.surface.remoteWidth),
                              (available.height - 45) / CGFloat(host.surface.remoteHeight)))
        host.window.setContentSize(NSSize(width: CGFloat(host.surface.remoteWidth) * ratio,
                                         height: CGFloat(host.surface.remoteHeight) * ratio))
        host.window.center()
    }
    static func start(_ value: AppDelegate) {
        delegate = value
        tick = Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) { _ in poll() }
        DispatchQueue.main.asyncAfter(deadline: .now() + 220) { finish(error: "probe timed out") }
    }
    static func rect(_ value: CGRect) -> [Double] {
        [Double(value.minX), Double(value.minY), Double(value.width), Double(value.height)]
    }
    static func state() {
        let windows: [[String: Any]] = delegate?.windows.values.map { host in
            ["id": host.id, "parent": delegate?.parents[host.id] as Any? ?? NSNull(),
             "kind": host.presentation?.kind ?? "unknown", "visible": host.window.isVisible,
             "remote_visible": host.remoteVisible, "borderless": host.window.styleMask == .borderless,
             "width": host.surface.remoteWidth, "height": host.surface.remoteHeight,
             "scale": host.rendering.scale, "frame": rect(host.window.frame),
             "source_rect": host.presentation.map { rect($0.rect) } ?? [],
             "image_rect": rect(host.surface.imageRect)]
        } ?? []
        let value: [String: Any] = ["phase": phase, "count": count, "windows": windows,
                                   "input_notice": delegate?.desktopInputNotice as Any? ?? NSNull()]
        if let data = try? JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]) {
            try? data.write(to: URL(fileURLWithPath: output + ".state.json"), options: .atomic)
        }
    }
    static func poll() {
        guard !finishing, let app = delegate else { return }
        if rootID == nil, let root = app.windows.values.first(where: { app.parents[$0.id] == nil && !$0.popup && $0.surface.image != nil }) {
            rootID = root.id; phase = "ready"; phaseStarted = now()
        }
        state()
        if phase == "opening", now() - phaseStarted > 8_000_000_000 { finish(error: "no matching popup draw"); return }
        if phase == "closing" {
            if let id = openedID, app.windows[id]?.window.isVisible == true {
                if now() - phaseStarted > 5_000_000_000 { finish(error: "test menu did not close") }
                return
            }
            openedID = nil
            if preview { phase = "ready"; preview = false; return }
            if count >= samples + 2 { finish(error: nil); return }
            phase = "between"
            DispatchQueue.main.asyncAfter(deadline: .now() + .milliseconds(Int.random(in: 160...260))) { openMenu() }
        }
        guard let data = try? Data(contentsOf: URL(fileURLWithPath: command)),
              let request = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let nextSerial = request["serial"] as? Int, nextSerial != serial else { return }
        serial = nextSerial
        if request["action"] as? String == "stop" { finish(error: "stopped by operator"); return }
        if phase == "preview", request["action"] as? String == "close" { closeMenu(); return }
        guard phase == "ready", let root = rootID.flatMap({ app.windows[$0] }),
              let x = request["x"] as? Double, let y = request["y"] as? Double,
              x >= 0, y >= 0, x < Double(root.surface.remoteWidth), y < Double(root.surface.remoteHeight),
              ["preview", "run"].contains(request["action"] as? String ?? "") else { return }
        guard !app.windows.values.contains(where: { $0.id != root.id && $0.remoteVisible && $0.window.isVisible }) else {
            finish(error: "an existing related window must be closed before the test"); return
        }
        point = CGPoint(x: x, y: y); preview = request["action"] as? String == "preview"
        samples = min(100, max(1, request["samples"] as? Int ?? 30)); count = 0; rows.removeAll()
        openMenu()
    }
    static func openMenu() {
        guard !finishing, let id = rootID, let host = delegate?.windows[id] else { return }
        let image = host.surface.imageRect
        let local = CGPoint(x: image.minX + point.x / CGFloat(host.surface.remoteWidth) * image.width,
                            y: image.minY + point.y / CGFloat(host.surface.remoteHeight) * image.height)
        let location = host.surface.convert(local, to: nil)
        let event = NSEvent.mouseEvent(with: .rightMouseDown, location: location, modifierFlags: [],
            timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: host.window.windowNumber,
            context: nil, eventNumber: count + 1, clickCount: 1, pressure: 1)!
        count += 1; phase = "opening"; phaseStarted = now()
        lock.lock(); current = ["count": count, "start": phaseStarted, "warmup": count <= 2,
                               "root_width": host.surface.remoteWidth, "root_height": host.surface.remoteHeight,
                               "root_scale": host.rendering.scale]; lock.unlock()
        injecting = true; host.surface.rightMouseDown(with: event); host.surface.rightMouseUp(with: event); injecting = false
    }
    static func closeMenu() {
        guard let id = rootID, let host = delegate?.windows[id] else { return }
        phase = "closing"; phaseStarted = now()
        let event = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [],
            timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: host.window.windowNumber,
            context: nil, characters: "\u{1b}", charactersIgnoringModifiers: "\u{1b}", isARepeat: false, keyCode: 53)!
        injecting = true; host.surface.keyDown(with: event); host.surface.keyUp(with: event); injecting = false
    }
    static func written(_ line: Data, start: UInt64, end: UInt64) {
        guard let value = try? JSONSerialization.jsonObject(with: line) as? [String: Any],
              value["type"] as? String == "input.pointer", value["button"] as? Int == 3,
              value["pressed"] as? Bool == true else { return }
        lock.lock(); current?["write_start"] = start; current?["write_end"] = end; lock.unlock()
    }
    static func selected(_ image: CGImage, object: [String: Any]) {
        if frames.count > 64 { frames.removeAll() }
        var value = object; value["selected"] = now(); frames[ObjectIdentifier(image)] = value
    }
    static func drew(_ image: CGImage) {
        let completed = now()
        guard phase == "opening", let frame = frames[ObjectIdentifier(image)],
              let received = unsignedInteger(frame["received"]), received >= phaseStarted,
              let number = unsignedInteger(frame["window_id"]), number <= UInt32.max,
              let app = delegate, let host = app.windows[UInt32(number)], host.id != rootID,
              app.parents[host.id] == rootID, host.remoteVisible, host.window.isVisible else { return }
        openedID = host.id
        guard host.popup && host.window.styleMask == .borderless else {
            finish(error: "related popup is still decorated; check active bridge version"); return
        }
        lock.lock(); guard let pending = current else { lock.unlock(); return }
        var row = frame; pending.forEach { row[$0] = $1 }; row["draw_end"] = completed
        row["borderless"] = true; row["can_become_key"] = host.window.canBecomeKey
        row["local_frame"] = rect(host.window.frame); row["source_rect"] = host.presentation.map { rect($0.rect) }
        row["kind"] = host.presentation?.kind; row["parent_id"] = app.parents[host.id]
        rows.append(row); current = nil; lock.unlock()
        if preview { phase = "preview"; state() }
        else { phase = "drawn"; DispatchQueue.main.asyncAfter(deadline: .now() + 0.15) { closeMenu() } }
    }
    static func finish(error: String?) {
        guard !finishing else { return }; finishing = true; tick?.invalidate()
        lock.lock(); let values = rows; lock.unlock()
        var result: [String: Any] = ["frames": values, "expected_samples": samples,
            "endpoint": "context menu AppKit draw completion", "injection": "SurfaceView rightMouseDown",
            "source_geometry": "preserved", "phase": phase]
        if let error { result["error"] = error }
        if let data = try? JSONSerialization.data(withJSONObject: result, options: [.sortedKeys]) {
            try? data.write(to: URL(fileURLWithPath: output), options: .atomic)
        }
        let cleanupMenu = openedID.flatMap { delegate?.windows[$0] }?.window.isVisible == true
        if cleanupMenu { closeMenu() }
        DispatchQueue.main.asyncAfter(deadline: .now() + (cleanupMenu ? 0.3 : 0)) {
            delegate?.stopping = true; delegate?.connected = false; delegate?.transport.disconnect()
            exit(error == nil ? 0 : 1)
        }
    }
}
