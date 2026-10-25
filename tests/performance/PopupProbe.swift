// Read-only application exercise: open a context menu at an observed location,
// then dismiss that test-owned menu with Escape. No production source changes.
import AppKit
import CoreImage

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
    static var menuRect: CGRect?
    static var baseline: [UInt8]?
    static var menuReference: [UInt8]?
    static var sceneClosed = false
    static var sceneMode: Bool { rootID.flatMap { delegate?.windows[$0]?.sessionMode } == true }
    static var expectedPopup: (width: Int, height: Int)?
    static var windowsBeforeInput = Set<UInt32>()
    static var ignoredPopupIDs = Set<UInt32>()
    static var tick: Timer?
    static let output = ProcessInfo.processInfo.environment["PROBE_OUTPUT"]!
    static let command = ProcessInfo.processInfo.environment["PROBE_COMMAND"]!
    static func now() -> UInt64 { DispatchTime.now().uptimeNanoseconds }

    static func configure(_ host: HostedWindow) {
        // Keep geometry supplied by the normal attach path; don't request a
        // separate benchmark resolution. Attach itself may change output/DPI.
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
             "remote_visible": host.remoteVisible, "application_scene": host.sessionMode, "borderless": host.window.styleMask == .borderless,
             "visible_rect": rect(host.surface.visibleRect), "document_rect": rect(host.surface.bounds),
             "local_scroll": host.surface.scrollOverflowLocally,
             "work_area": host.window.screen.map { rect($0.visibleFrame) } ?? [],
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
    static func snapshot() throws {
        // Explicit, local-only diagnostic export outside timed samples. These
        // are decoded stream images, not a screenshot of the Mac desktop.
        let directory = URL(fileURLWithPath: output + ".snapshot", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true,
                                                attributes: [.posixPermissions: 0o700])
        for host in delegate?.windows.values ?? Dictionary<UInt32, HostedWindow>().values {
            guard host.window.isVisible, let image = host.surface.image,
                  let data = NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:]) else { continue }
            let file = directory.appendingPathComponent("window-\(host.id).png")
            try data.write(to: file, options: .atomic)
            try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: file.path)
            if let viewport = host.overflowViewport,
               let bitmap = viewport.bitmapImageRepForCachingDisplay(in: viewport.bounds) {
                viewport.cacheDisplay(in: viewport.bounds, to: bitmap)
                if let data = bitmap.representation(using: .png, properties: [:]) {
                    let visible = directory.appendingPathComponent("window-\(host.id)-viewport.png")
                    try data.write(to: visible, options: .atomic)
                    try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: visible.path)
                }
            }
        }
        state()
        let metadata = try Data(contentsOf: URL(fileURLWithPath: output + ".state.json"))
        try metadata.write(to: directory.appendingPathComponent("state.json"), options: .atomic)
    }
    static func poll() {
        guard !finishing, let app = delegate else { return }
        if rootID == nil, let root = app.windows.values.first(where: { app.parents[$0.id] == nil && !$0.popup && $0.surface.image != nil }) {
            rootID = root.id; phase = "ready"; phaseStarted = now()
        }
        state()
        if phase == "opening", now() - phaseStarted > 8_000_000_000 { finish(error: "no matching popup draw"); return }
        if phase == "closing" {
            if sceneMode ? !sceneClosed : (openedID.flatMap { app.windows[$0]?.window.isVisible } == true) {
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
        if request["action"] as? String == "snapshot", ["ready", "preview"].contains(phase) {
            do { try snapshot() } catch { finish(error: "snapshot export failed") }
            return
        }
        if request["action"] as? String == "scroll", ["ready", "preview"].contains(phase),
           let id = request["window_id"] as? UInt32, let host = app.windows[id], host.surface.scrollOverflowLocally,
           let delta = request["delta"] as? Int32, abs(Int64(delta)) <= 2000,
           let cg = CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 1, wheel1: delta, wheel2: 0, wheel3: 0),
           let event = NSEvent(cgEvent: cg) {
            injecting = true; host.surface.scrollWheel(with: event); injecting = false
            return
        }
        if ["hover", "wheel"].contains(request["action"] as? String ?? ""), ["ready", "preview"].contains(phase),
           let id = request["window_id"] as? UInt32, let host = app.windows[id],
           let x = request["x"] as? Double, let y = request["y"] as? Double,
           x.isFinite, y.isFinite, x >= 0, y >= 0, x < Double(host.surface.remoteWidth), y < Double(host.surface.remoteHeight) {
            let rect = host.surface.imageRect
            let point = CGPoint(x: rect.minX + x / Double(host.surface.remoteWidth) * rect.width,
                                y: rect.minY + y / Double(host.surface.remoteHeight) * rect.height)
            guard host.surface.visibleRect.contains(point) else { return }
            if request["action"] as? String == "wheel" {
                guard !host.surface.scrollOverflowLocally else { return }
                let button = (request["up"] as? Bool ?? false) ? 4 : 5
                for pressed in [true, false] {
                    app.send(["type": "input.pointer", "window_id": id, "x": Int(x), "y": Int(y),
                              "button": button, "pressed": pressed])
                }
                return
            }
            let event = NSEvent.mouseEvent(with: .mouseMoved, location: host.surface.convert(point, to: nil), modifierFlags: [],
                timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: host.window.windowNumber, context: nil,
                eventNumber: 0, clickCount: 0, pressure: 0)!
            injecting = true; host.surface.mouseMoved(with: event); injecting = false
            return
        }
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
        if sceneMode {
            guard let values = request["menu_rect"] as? [Double], values.count == 4,
                  values.allSatisfy({ $0.isFinite }), values[0] >= 0, values[1] >= 0,
                  values[2] >= 8, values[3] >= 8,
                  values[0] + values[2] <= Double(root.surface.remoteWidth),
                  values[1] + values[3] <= Double(root.surface.remoteHeight) else {
                finish(error: "scene probe requires an observed menu_rect ROI in source pixels"); return
            }
            let roi = CGRect(x: values[0], y: values[1], width: values[2], height: values[3])
            if menuRect != roi { menuReference = nil }
            menuRect = roi
            if !preview && menuReference == nil { finish(error: "preview the observed menu ROI before timing"); return }
            expectedPopup = nil
        } else if !preview {
            guard let width = request["popup_width"] as? Int, let height = request["popup_height"] as? Int,
                  width > 0, height > 0 else {
                finish(error: "run requires observed popup_width and popup_height in stream pixels"); return
            }
            expectedPopup = (width, height)
        } else { expectedPopup = nil }
        ignoredPopupIDs.removeAll()
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
        if sceneMode { baseline = host.surface.displayFrame.flatMap { signature($0) } }
        count += 1; phase = "opening"; phaseStarted = now()
        windowsBeforeInput = Set(delegate?.windows.keys ?? Dictionary<UInt32, HostedWindow>().keys)
        lock.lock(); current = ["count": count, "start": phaseStarted, "warmup": count <= 2,
                               "root_width": host.surface.remoteWidth, "root_height": host.surface.remoteHeight,
                               "root_scale": host.rendering.scale]; lock.unlock()
        injecting = true; host.surface.rightMouseDown(with: event); host.surface.rightMouseUp(with: event); injecting = false
    }
    static func closeMenu() {
        guard let id = rootID, let host = delegate?.windows[id] else { return }
        if sceneMode && phase == "preview", let image = host.surface.displayFrame {
            menuReference = signature(image) // Use the inspected, fully painted preview.
        }
        sceneClosed = false
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
    static let roiContext = CIContext(options: [.cacheIntermediates: false])
    static func signature(_ frame: VideoFrame) -> [UInt8]? {
        guard let buffer = frame.pixelBuffer else { return frame.image.flatMap { signature($0) } }
        guard let roi = menuRect else { return nil }
        // CoreImage uses bottom-left coordinates. The protocol/CGImage ROI is
        // top-left. Read just a 16x16 GPU thumbnail, never the whole video frame.
        let rect = CGRect(x: roi.minX, y: CGFloat(CVPixelBufferGetHeight(buffer)) - roi.maxY,
                          width: roi.width, height: roi.height)
        let thumbnail = CIImage(cvPixelBuffer: buffer).cropped(to: rect)
            .transformed(by: CGAffineTransform(translationX: -rect.minX, y: -rect.minY))
            .transformed(by: CGAffineTransform(scaleX: 16 / rect.width, y: 16 / rect.height))
        guard let image = roiContext.createCGImage(thumbnail, from: CGRect(x: 0, y: 0, width: 16, height: 16)) else { return nil }
        return signaturePixels(image)
    }
    static func signature(_ image: CGImage) -> [UInt8]? {
        guard let roi = menuRect, let crop = image.cropping(to: roi) else { return nil }
        return signaturePixels(crop)
    }
    static func signaturePixels(_ crop: CGImage) -> [UInt8]? {
        var bytes = [UInt8](repeating: 0, count: 16 * 16 * 4)
        let drawn = bytes.withUnsafeMutableBytes { buffer -> Bool in
            guard let context = CGContext(data: buffer.baseAddress, width: 16, height: 16, bitsPerComponent: 8,
                bytesPerRow: 16 * 4, space: CGColorSpaceCreateDeviceRGB(),
                bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return false }
            context.draw(crop, in: CGRect(x: 0, y: 0, width: 16, height: 16))
            return true
        }
        return drawn ? bytes : nil
    }
    static func distance(_ one: [UInt8], _ two: [UInt8]) -> Double {
        guard one.count == two.count && !one.isEmpty else { return .infinity }
        return Double(zip(one, two).reduce(0) { $0 + abs(Int($1.0) - Int($1.1)) }) / Double(one.count)
    }
    static func observedVideo(_ frame: VideoFrame, object: [String: Any], completed: UInt64) {
        if sceneMode {
            guard ["opening", "closing"].contains(phase), let pixels = signature(frame) else { return }
            drewScene(pixels, frame: object, completed: completed)
        } else if let image = frame.image {
            frames[ObjectIdentifier(image)] = object
            drew(image, timestamp: completed)
        }
    }
    static func drewScene(_ image: CGImage, completed: UInt64) {
        guard let frame = frames[ObjectIdentifier(image)], let pixels = signature(image) else { return }
        drewScene(pixels, frame: frame, completed: completed)
    }
    static func drewScene(_ pixels: [UInt8], frame: [String: Any], completed: UInt64) {
        guard ["opening", "closing"].contains(phase),
              unsignedInteger(frame["window_id"]) == rootID.map(UInt64.init),
              let received = unsignedInteger(frame["received"]), received >= phaseStarted,
              let baseline else { return }
        let changed = distance(pixels, baseline)
        if phase == "closing" {
            let separation = menuReference.map { distance($0, baseline) } ?? 12
            if changed < max(2, separation * 0.2) { sceneClosed = true }
            return
        }
        if let reference = menuReference, !preview {
            let separation = distance(reference, baseline)
            guard separation > 3, changed > separation * 0.5,
                  distance(pixels, reference) < max(2, separation * 0.25) else { return }
        } else {
            guard changed > 3 else { return }
            menuReference = pixels
        }
        lock.lock(); guard let pending = current else { lock.unlock(); return }
        var row = frame; pending.forEach { row[$0] = $1 }; row["draw_end"] = completed
        row["detection"] = "composed menu ROI matched preview"; row["menu_rect"] = menuRect.map { rect($0) }
        row["baseline_distance"] = changed; row["scene_window_count"] = delegate?.windows.count
        rows.append(row); current = nil; lock.unlock()
        openedID = rootID
        if preview { phase = "preview"; state() }
        else { phase = "drawn"; DispatchQueue.main.asyncAfter(deadline: .now() + 0.15) { closeMenu() } }
    }
    static func drew(_ image: CGImage, timestamp: UInt64? = nil) {
        let completed = timestamp ?? now()
        if sceneMode { drewScene(image, completed: completed); return }
        guard phase == "opening", let frame = frames[ObjectIdentifier(image)],
              let received = unsignedInteger(frame["received"]), received >= phaseStarted,
              let number = unsignedInteger(frame["window_id"]), number <= UInt32.max,
              let app = delegate, let host = app.windows[UInt32(number)], host.id != rootID,
              app.parents[host.id] == rootID, host.remoteVisible, host.window.isVisible else { return }
        // Some applications expose hover tips as popup menus too. Match the
        // observed target, and never time a repaint of a pre-existing child.
        guard !windowsBeforeInput.contains(host.id) else { return }
        if let expected = expectedPopup,
           host.presentation?.kind != "menu" || image.width != expected.width || image.height != expected.height {
            ignoredPopupIDs.insert(host.id); return
        }
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
        if error != nil, ProcessInfo.processInfo.environment["PROBE_SNAPSHOT_ON_FAILURE"] == "1" {
            try? snapshot() // Explicit local opt-in; outside the measured samples.
        }
        var result: [String: Any] = ["frames": values, "expected_samples": samples,
            "endpoint": "context menu AppKit draw completion", "injection": "SurfaceView rightMouseDown",
            "source_geometry": "normal attach; no benchmark resolution override", "phase": phase,
            "ignored_popup_count": ignoredPopupIDs.count]
        if sceneMode {
            result["source_geometry"] = "fixed application scene canvas"
            result["detection"] = "observed menu ROI compared with preview and closed baseline"
            result["menu_rect"] = menuRect.map { rect($0) }
        }
        if let expected = expectedPopup { result["expected_popup_pixels"] = [expected.width, expected.height] }
        if let error { result["error"] = error }
        if let data = try? JSONSerialization.data(withJSONObject: result, options: [.sortedKeys]) {
            try? data.write(to: URL(fileURLWithPath: output), options: .atomic)
        }
        // A failed ROI match can still leave a real menu open. Always dismiss
        // an attempted menu on this authorized root before detaching.
        let cleanupMenu = rootID != nil && ["opening", "preview", "drawn", "closing"].contains(phase)
        if cleanupMenu { closeMenu() }
        DispatchQueue.main.asyncAfter(deadline: .now() + (cleanupMenu ? 0.3 : 0)) {
            delegate?.stopping = true; delegate?.connected = false; delegate?.transport.disconnect()
            exit(error == nil ? 0 : 1)
        }
    }
}
