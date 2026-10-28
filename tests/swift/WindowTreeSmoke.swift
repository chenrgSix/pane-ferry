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
        // Reproduce a popup taller than the receiving work area. Keep readable
        // pixels, expose the bottom via a real AppKit clip view, and exercise
        // input after scrolling rather than merely testing a clamp formula.
        let scrollRoot = update(metadata(20, nil, "window", CGRect(x: -600, y: 200, width: 1000, height: 500)))
        RunLoop.main.run(until: Date().addingTimeInterval(0.05))
        scrollRoot.applyingRemote = true
        scrollRoot.window.setContentSize(NSSize(width: 800, height: 600))
        scrollRoot.window.setFrameOrigin(NSPoint(x: 100, y: 100))
        scrollRoot.applyingRemote = false
        let workArea = (scrollRoot.window.screen ?? NSScreen.main!).visibleFrame
        let tall = update(metadata(10, 20, "menu", CGRect(x: -450, y: 350, width: 200, height: 2000), 1))
        let scroll = tall.overflowViewport!
        precondition(workArea.insetBy(dx: -1, dy: -1).contains(tall.window.frame), "overlay stays above Dock and below menu bar")
        precondition(abs(tall.surface.frame.height - 1600) < 1 && abs(tall.surface.frame.width - 160) < 1,
                     "overflow must not shrink the source text")
        precondition(scroll.hasVerticalScroller && !scroll.hasHorizontalScroller)
        precondition(scroll.contentView.bounds.minY == 0, "a new menu starts at its first item")
        let nested = update(metadata(11, 10, "menu", CGRect(x: -250, y: 900, width: 100, height: 40), 1))
        let peer = update(metadata(12, 20, "menu", CGRect(x: -250, y: 900, width: 100, height: 40), 1, order: 1))
        precondition(peer.window.frame == nested.window.frame, "flat and nested transient trees share the same source scene")
        let nestedBefore = nested.window.frame
        scroll.contentView.scroll(to: CGPoint(x: 0, y: 120)); scroll.reflectScrolledClipView(scroll.contentView)
        app.attachParents()
        precondition(abs(nested.window.frame.minY - nestedBefore.minY - 120) < 1,
                     "nested popup follows its scrolled source anchor")
        precondition(peer.window.frame == nested.window.frame, "flat sibling menus follow the same viewport offset")
        let retained = scroll.contentView.bounds.origin
        tall.present(pixels(tall.surface.remoteWidth, tall.surface.remoteHeight), mediaMode: "fixture")
        app.attachParents()
        precondition(scroll.contentView.bounds.origin == retained, "video frames must not reset local scrolling")
        let wheel = CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 1, wheel1: -80, wheel2: 0, wheel3: 0)!
        messages.removeAll()
        tall.surface.scrollWheel(with: NSEvent(cgEvent: wheel)!)
        RunLoop.main.run(until: Date().addingTimeInterval(0.05))
        precondition(scroll.contentView.bounds.minY > retained.y, "a real wheel event scrolls the local viewport")
        precondition(!messages.contains { $0["type"] as? String == "input.pointer" }, "local scroll must not also scroll the remote menu")
        scroll.contentView.scroll(to: CGPoint(x: 0, y: tall.surface.bounds.height - scroll.contentSize.height))
        scroll.reflectScrolledClipView(scroll.contentView)
        // The owner can finish its initial screen placement while AppKit is
        // processing the wheel event. Target source row 1990 at the current
        // document size, rather than assuming it is still 1600 points tall.
        let bottomPoint = tall.surface.convert(CGPoint(x: tall.surface.bounds.midX,
            y: tall.surface.bounds.maxY - tall.surface.bounds.height * 10 / 2000), to: nil)
        precondition(scroll.contentView.bounds.contains(scroll.contentView.convert(bottomPoint, from: nil)),
                     "the tested source row must actually be visible after scrolling")
        let bottomEvent = NSEvent.mouseEvent(with: .leftMouseDown, location: bottomPoint,
            modifierFlags: [], timestamp: 0, windowNumber: tall.window.windowNumber, context: nil,
            eventNumber: 2, clickCount: 1, pressure: 1)!
        tall.surface.mouseDown(with: bottomEvent); tall.surface.mouseUp(with: bottomEvent)
        let bottomPress = messages.first { $0["type"] as? String == "input.pointer" && $0["pressed"] as? Bool == true }!
        // AppKit's document/window conversion can land just below an integer
        // pixel boundary on older SDKs; source coordinates truncate to pixels.
        let bottomY = bottomPress["y"] as? Int ?? -1
        precondition(bottomPress["window_id"] as? UInt32 == 10 && abs(bottomY - 1990) <= 1,
                     "clicking the visible bottom item must reach its source row: \(bottomPress), frame=\(tall.surface.frame), bounds=\(tall.surface.bounds)")
        precondition(!messages.contains {
            [10, 11, 12].contains($0["window_id"] as? UInt32 ?? 0) &&
                ["window.focus", "window.configure"].contains($0["type"] as? String ?? "")
        }, "overlay layout/scroll never focuses or configures source popups")
        tall.applyRemoteVisibility(false); tall.applyRemoteVisibility(true); app.attachParents()
        precondition(scroll.contentView.bounds.minY == 0, "a hidden/reopened menu must not retain the previous bottom offset")
        // Shrinking back to an ordinary menu removes the viewport and stale offset.
        _ = update(metadata(10, 20, "menu", CGRect(x: -450, y: 350, width: 200, height: 100), 1))
        precondition(tall.overflowViewport == nil && tall.window.contentView === tall.surface && !tall.surface.scrollOverflowLocally)
        precondition(abs(tall.surface.frame.height - 80) < 1)
        for id: UInt32 in [10, 11, 12, 20] { app.windows.removeValue(forKey: id)?.closeLocally(); app.parents.removeValue(forKey: id) }
        // Negative display origins and horizontal overflow use the same policy.
        let secondary = CGRect(x: -1600, y: -200, width: 1600, height: 900)
        let both = OverlayViewport(content: CGRect(x: -1700, y: -900, width: 2000, height: 1800), available: secondary)
        precondition(both.frame == secondary && both.scrolls && both.documentSize == CGSize(width: 2000, height: 1800))
        let shift = OverlayViewport(content: CGRect(x: -50, y: -250, width: 200, height: 100), available: secondary)
        precondition(shift.frame == CGRect(x: -200, y: -200, width: 200, height: 100) && !shift.scrolls)
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
        for seq in 1...3 {
            _=update(metadata(99,1,"dialog",CGRect(x:0,y:0,width:20,height:20)))
            app.receive(Incoming(object:["type":"window.destroyed","window_id":99,"session_id":"fixture","seq":seq],image:nil))
            precondition(app.windows[99]==nil)
        }
        let reused=update(metadata(99,1,"dialog",CGRect(x:0,y:0,width:20,height:20)))
        precondition(app.windows[99] === reused)
        app.capabilities.insert("video.qos")
        app.receive(Incoming(object:["type":"frame","window_id":99,"session_id":"fixture","seq":4,"geometry_revision":1,"frame_id":1],image:pixels(40,40)))
        precondition(app.pendingVideoFeedbackID==nil, "scheduling a redraw is not feedback")
        let bitmap=reused.surface.bitmapImageRepForCachingDisplay(in:reused.surface.bounds)!
        reused.surface.cacheDisplay(in:reused.surface.bounds,to:bitmap)
        precondition(app.pendingVideoFeedbackID==1, "feedback follows the actual AppKit draw")
        let delayedVideo = pixels(40, 40)
        app.receive(Incoming(object:["type":"input.authorization", "session_id":"fixture", "seq":6, "state":"ready"], image:nil))
        app.receive(Incoming(object:["type":"frame", "window_id":99, "session_id":"fixture", "seq":5, "geometry_revision":1, "frame_id":2], image:delayedVideo))
        precondition(reused.surface.image === delayedVideo, "control overtaking decode must not suppress a still-current frame")
        app.receive(Incoming(object:["type":"frame", "window_id":99, "session_id":"fixture", "seq":4, "geometry_revision":1], image:pixels(40,40)))
        precondition(reused.surface.image === delayedVideo, "older video from the same window is still rejected")
        var sessionData = metadata(100, nil, "window", CGRect(x: 0, y: 0, width: 800, height: 450))
        sessionData["surface_type"] = "session"; sessionData["role"] = "session"
        let scene = update(sessionData)
        precondition(scene.sessionMode && scene.fixedCanvas && !scene.desktopMode)
        messages.removeAll()
        scene.window.setContentSize(NSSize(width: 700, height: 400))
        scene.windowDidResize(Notification(name: NSWindow.didResizeNotification))
        RunLoop.main.run(until: Date().addingTimeInterval(0.3))
        precondition(!messages.contains { $0["type"] as? String == "window.configure" && ($0["window_id"] as? UInt32) == 100 })
        precondition(scene.window.subtitle.contains("应用会话画面"))
        for host in app.windows.values { host.closeLocally() }
        app.windows.removeAll(); app.connected = false; app.stopping = true
        print("PASS: source-relative nested overlays, fractional density, letterboxing, screen bounds, native overflow scrolling, bottom-item input, nested scroll anchors, passive tooltip hit testing, focus/resize isolation, dialogs, late parents, hidden/deleted owners and cycles")
    }
}
