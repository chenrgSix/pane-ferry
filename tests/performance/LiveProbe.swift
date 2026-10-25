// Opt-in hooks for the installed Agent and production QUIC path; no server instrumentation.
// Synthetic test input only; never reads the controller clipboard.
import AppKit

enum Probe {
    static let lock = NSLock()
    static var rows: [[String: Any]] = []
    static var current: [String: Any]?
    static var frames: [ObjectIdentifier: [String: Any]] = [:]
    static var delegate: AppDelegate?
    static var started = false
    static var count = 0
    static var injecting = false
    static let samples = Int(ProcessInfo.processInfo.environment["PROBE_SAMPLES"] ?? "30")!
    static let width = Int(ProcessInfo.processInfo.environment["PROBE_WIDTH"]!)!
    static let height = Int(ProcessInfo.processInfo.environment["PROBE_HEIGHT"]!)!
    static let output = ProcessInfo.processInfo.environment["PROBE_OUTPUT"]!
    static func now() -> UInt64 { DispatchTime.now().uptimeNanoseconds }

    static func configure(_ host: HostedWindow) {
        host.fixedPixels = (width, height)
        let available = (host.window.screen ?? NSScreen.main!).visibleFrame.size
        let ratio = min(1 / host.window.backingScaleFactor,
                        min(available.width / CGFloat(width), (available.height - 30) / CGFloat(height)))
        host.window.setContentSize(NSSize(width: CGFloat(width) * ratio, height: CGFloat(height) * ratio))
        host.window.center()
    }
    static func start(_ value: AppDelegate) {
        delegate = value
        DispatchQueue.main.asyncAfter(deadline: .now() + 90) { finish(error: "measurement timed out") }
        waitForWindow()
    }
    static func waitForWindow() {
        guard let host = delegate?.windows.values.first, host.surface.image != nil,
              host.surface.remoteWidth == width, host.surface.remoteHeight == height else {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) { waitForWindow() }; return
        }
        guard !started else { return }; started = true
        // Exclude transport startup, first keyframe and decoder allocation from steady-state samples.
        DispatchQueue.main.asyncAfter(deadline: .now() + 2) { next() }
    }
    static func written(_ line: Data, start: UInt64, end: UInt64) {
        guard let object = try? JSONSerialization.jsonObject(with: line) as? [String: Any],
              let type = object["type"] as? String,
              type == "input.text.commit" || ((type == "input.key" || type == "input.pointer") && object["pressed"] as? Bool == true)
        else { return }
        lock.lock(); defer { lock.unlock() }
        current?["write_start"] = start; current?["write_end"] = end
    }
    static func selected(_ image: CGImage, object: [String: Any]) {
        lock.lock(); defer { lock.unlock() }
        if frames.count > 64 { frames.removeAll() }
        var value = object
        value["selected"] = now()
        frames[ObjectIdentifier(image)] = value
    }
    static func marker(_ image: CGImage) -> Int? {
        guard let crop = image.cropping(to: CGRect(x: 0, y: 16, width: 256, height: 1)) else { return nil }
        var pixels = [UInt8](repeating: 0, count: 256 * 4)
        return pixels.withUnsafeMutableBytes { bytes in
            guard let context = CGContext(data: bytes.baseAddress, width: 256, height: 1,
                bitsPerComponent: 8, bytesPerRow: 256 * 4, space: CGColorSpaceCreateDeviceRGB(),
                bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return nil }
            context.draw(crop, in: CGRect(x: 0, y: 0, width: 256, height: 1))
            let data = bytes.bindMemory(to: UInt8.self)
            var value = 0
            for bit in 0..<8 {
                if data[(16 + bit * 32) * 4] > 128 { value |= 1 << bit }
            }
            return value
        }
    }
    static func observedVideo(_ frame: VideoFrame, object: [String: Any], completed: UInt64) {
        guard current != nil, let image = frame.image else { return }
        lock.lock(); frames[ObjectIdentifier(image)] = object; lock.unlock()
        drew(image, timestamp: completed)
    }
    static func drew(_ image: CGImage, timestamp: UInt64? = nil) {
        let completed = timestamp ?? now() // End of actual NSImage.draw; sampling overhead is excluded.
        guard let marker = marker(image) else { return }
        lock.lock()
        if let expected = current?["count"] as? Int, marker > expected {
            lock.unlock(); finish(error: "fixture emitted an unexpected input marker"); return
        }
        guard let pending = current, pending["count"] as? Int == marker,
              var frame = frames[ObjectIdentifier(image)] else { lock.unlock(); return }
        for (key, value) in pending { frame[key] = value }
        frame["draw_end"] = completed
        rows.append(frame); current = nil
        lock.unlock()
        DispatchQueue.main.asyncAfter(deadline: .now() + .milliseconds(Int.random(in: 80...145))) { next() }
    }
    static func next() {
        guard let host = delegate?.windows.values.first else { finish(error: "test window disappeared"); return }
        if count == (samples + 2) * 2 { finish(error: nil); return }
        count += 1
        let mode = ["key", "pointer"][(count - 1) % 2]
        lock.lock()
        current = ["count":count, "mode":mode, "start":now(), "warmup":count <= 4]
        lock.unlock()
        injecting = true
        defer { injecting = false }
        switch mode {
        case "key":
            let event = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [],
                timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: host.window.windowNumber,
                context: nil, characters: "\u{f702}", charactersIgnoringModifiers: "\u{f702}", isARepeat: false, keyCode: 123)!
            host.surface.keyDown(with: event)
            host.surface.keyUp(with: event)
        default:
            let event = NSEvent.mouseEvent(with: .leftMouseDown,
                location: NSPoint(x: host.surface.bounds.midX, y: host.surface.bounds.midY), modifierFlags: [],
                timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: host.window.windowNumber,
                context: nil, eventNumber: count, clickCount: 1, pressure: 1)!
            host.surface.mouseDown(with: event)
            host.surface.mouseUp(with: event)
        }
    }
    static func finish(error: String?) {
        lock.lock(); let values = rows; lock.unlock()
        var result: [String: Any] = ["frames":values, "width":width, "height":height]
        result["expected_samples_per_mode"] = samples
        result["pending"] = current
        result["last_marker"] = delegate?.windows.values.first?.surface.image.flatMap { marker($0) }
        result["session_id"] = delegate?.session
        result["endpoint"] = "AppKit draw completion"
        result["injection"] = "SurfaceView input handlers"
        result["ready"] = delegate?.ready ?? false
        result["alive"] = delegate?.transport.alive ?? false
        result["window_count"] = delegate?.windows.count ?? 0
        result["sequence"] = delegate?.lastSequence ?? 0
        if let error = error { result["error"] = error }
        do { try JSONSerialization.data(withJSONObject: result, options: [.sortedKeys]).write(to: URL(fileURLWithPath: output)) }
        catch { FileHandle.standardError.write(Data("Cannot save benchmark output\n".utf8)) }
        delegate?.stopping = true
        delegate?.connected = false
        delegate?.transport.disconnect()
        exit(error == nil ? 0 : 1)
    }
}
