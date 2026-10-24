import AppKit

@main struct WindowTreeSmoke {
    static func pixels(_ width: Int, _ height: Int) -> CGImage {
        CGContext(data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: width * 4,
                  space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue)!.makeImage()!
    }
    static func main() throws {
        _ = NSApplication.shared
        let options = try Options.parse(["--native-peer", "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", "--native-state", "/tmp/unused", "--native-helper", "/usr/bin/false", "--session", "fixture"])
        let app = AppDelegate(options: options, clipboard: NSPasteboard.withUniqueName())
        app.session = "fixture"; app.ready = true
        var messages: [[String: Any]] = []
        app.wireSink = { messages.append($0) }
        func metadata(_ id: UInt32, _ parent: UInt32?, _ kind: String, _ rect: CGRect, _ density: Double = 2, order: Int = 0) -> [String: Any] {
            ["window_id": id, "parent_id": parent.map { NSNumber(value: $0) } as Any, "title": "Generic fixture \(id)",
             "role": parent == nil ? "toplevel" : kind == "dialog" ? "dialog" : "popup",
             "width": Int(rect.width * density), "height": Int(rect.height * density), "scale_factor": density >= 2 ? 2 : 1,
             "geometry_revision": 1, "visible": true,
             "presentation": ["x": rect.minX, "y": rect.minY, "width": rect.width, "height": rect.height, "kind": kind, "order": order]]
        }
        func update(_ object: [String: Any]) -> HostedWindow {
            app.updateWindow(object, create: true)
            let host = app.windows[object["window_id"] as! UInt32]!
            host.present(pixels(host.surface.remoteWidth, host.surface.remoteHeight), mediaMode: "fixture")
            app.attachParents()
            return host
        }
        let root = update(metadata(1, nil, "window", CGRect(x: -600, y: 200, width: 1000, height: 500)))
        precondition(root.window.canBecomeKey && root.window.canBecomeMain)
        RunLoop.main.run(until: Date().addingTimeInterval(0.05)) // drain initial fitToScreen
        root.applyingRemote = true
        root.window.setContentSize(NSSize(width: 800, height: 600))
        root.window.setFrameOrigin(NSPoint(x: 100, y: 100))
        root.applyingRemote = false
        let menuData = metadata(2, 1, "menu", CGRect(x: -400, y: 300, width: 200, height: 80), order: 10)
        messages.removeAll()
        let menu = update(menuData)
        precondition(menu.window.styleMask == .borderless && !menu.window.canBecomeKey && !menu.window.canBecomeMain)
        precondition(menu.window.parent === root.window && menu.window.isVisible)
        precondition(!menu.window.ignoresMouseEvents)
        precondition(menu.surface.acceptsFirstMouse(for: nil))
        precondition(menu.surface.trackingAreas.contains { $0.options.contains(.activeInActiveApp) && $0.options.contains(.mouseMoved) })
        precondition(!messages.contains { ["window.focus", "window.configure"].contains($0["type"] as? String ?? "") }, "opening an overlay must not activate or resize it")
        let expected = root.window.convertToScreen(root.surface.convert(CGRect(x: 160, y: 180, width: 160, height: 64), to: nil))
        precondition(menu.window.frame.equalTo(expected), "source position includes parent letterboxing and top-left coordinate conversion")
        // A nested popup uses its own source density, not its parent's backing scale.
        let tooltip = update(metadata(3, 2, "tooltip", CGRect(x: -350, y: 320, width: 100, height: 20), 1.5))
        precondition(tooltip.window.parent === menu.window && tooltip.window.ignoresMouseEvents)
        precondition(abs(tooltip.surface.bounds.width - 80) < 0.01 && abs(tooltip.surface.bounds.height - 16) < 0.01)
        precondition(tooltip.window.isVisible && !tooltip.window.canBecomeKey)
        let click = NSEvent.mouseEvent(with: .leftMouseDown, location: NSPoint(x: menu.surface.bounds.midX, y: menu.surface.bounds.midY),
                                      modifierFlags: [], timestamp: 0, windowNumber: menu.window.windowNumber, context: nil,
                                      eventNumber: 1, clickCount: 1, pressure: 1)!
        menu.surface.mouseDown(with: click); menu.surface.mouseUp(with: click)
        let press = messages.first { $0["type"] as? String == "input.pointer" && $0["pressed"] as? Bool == true }!
        precondition(press["window_id"] as? UInt32 == 2 && press["x"] as? Int == 200 && press["y"] as? Int == 80,
                     "menu clicks retain child-local source-pixel coordinates")
        let dialog = update(metadata(4, 1, "dialog", CGRect(x: -550, y: 400, width: 300, height: 200)))
        precondition(!dialog.popup && dialog.window.styleMask.contains(.titled) && dialog.window.parent === root.window)
        precondition(dialog.window.canBecomeKey && dialog.window.isVisible)
        let outside = update(metadata(5, 2, "popup", CGRect(x: 300, y: 280, width: 300, height: 100), order: 20))
        precondition(outside.window.frame.maxX > root.window.frame.maxX, "menus may extend outside the owner's content bounds")
        // A parent can arrive after its child. No orphan window flashes at screen center.
        let orphan = update(metadata(6, 7, "menu", CGRect(x: 10, y: 10, width: 60, height: 40)))
        precondition(!orphan.window.isVisible)
        let late = update(metadata(7, 1, "menu", CGRect(x: 0, y: 0, width: 100, height: 100)))
        precondition(orphan.window.isVisible && orphan.window.parent === late.window)
        let before = menu.window.frame.origin
        root.window.setFrameOrigin(NSPoint(x: 150, y: 140)); app.attachParents()
        precondition(abs(menu.window.frame.minX - before.x - 50) < 0.01 && abs(menu.window.frame.minY - before.y - 40) < 0.01)
        root.applyRemoteVisibility(false); app.attachParents()
        precondition([menu, tooltip, dialog, outside, orphan, late].allSatisfy { !$0.window.isVisible })
        root.applyRemoteVisibility(true); app.attachParents()
        precondition(menu.window.isVisible && tooltip.window.isVisible)
        // Cycles and vanished parents hide descendants instead of reparenting them to the desktop.
        app.parents[2] = 3; app.attachParents()
        precondition(!menu.window.isVisible && !tooltip.window.isVisible)
        app.parents[2] = 1; app.attachParents()
        app.windows.removeValue(forKey: 2)?.closeLocally(); app.attachParents()
        precondition(!tooltip.window.isVisible && !outside.window.isVisible)
        let legacy = WindowPresentation.parse(["role": "popup", "x": 12, "y": -4], width: 100, height: 50)!
        precondition(legacy.rect == CGRect(x: 12, y: -4, width: 100, height: 50))
        precondition(WindowPresentation.parse(["presentation": ["x": Double.infinity, "y": 0, "width": 1, "height": 1]], width: 1, height: 1) == nil)
        precondition(WindowPresentation.parse(["presentation": ["kind": "menu"]], width: 1, height: 1) == nil)
        for host in app.windows.values { host.closeLocally() }
        app.windows.removeAll(); app.connected = false; app.stopping = true
        print("PASS: source-relative nested overlays, fractional density, letterboxing, overflow, passive tooltip hit testing, child-local input, focus/resize isolation, dialogs, late parents, hidden/deleted owners and cycles")
    }
}
