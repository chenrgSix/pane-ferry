import AppKit

@main
struct H264OnlySmoke {
    static func main() throws {
        let timing = VideoFeedbackTiming(frameID: 1, received: 0, decoding: 1000, decoded: 2_001_000, presented: 5_001_000)!
        let measured = timing.wire(at: 9_001_000)!
        precondition(measured == ["queue_us": 1, "decode_us": 2000, "present_us": 3000, "feedback_wait_us": 4000])
        precondition(VideoFeedbackTiming(frameID: 1, received: nil, decoding: 1, decoded: 2, presented: 3) == nil)
        precondition(VideoFeedbackTiming(frameID: 1, received: 3, decoding: 1, decoded: 2, presented: 4) == nil)
        precondition(timing.wire(at: 1) == nil && timing.wire(at: 6_000_000_000) == nil)
        _ = NSApplication.shared
        let args = ["--native-peer", "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", "--native-state", "/tmp/fixture",
                    "--native-helper", "/tmp/fixture/bridge", "--session", "retained"]
        let options = try Options.parse(args)
        precondition(options.requestedVideoFPS == 60)
        let thirtyOptions = try Options.parse(args + ["--fps", "30"])
        let sixtyOptions = try Options.parse(args + ["--fps", "60"])
        precondition(thirtyOptions.requestedVideoFPS == 30 && sixtyOptions.requestedVideoFPS == 60)
        for invalid in ["0", "90", "24", "invalid"] {
            do { _ = try Options.parse(args + ["--fps", invalid]); preconditionFailure("invalid frame rate accepted") }
            catch HostError.message { }
        }
        for deprecated in ["png", "h264"] {
            do { _ = try Options.parse(args + ["--quality", deprecated]); preconditionFailure("obsolete quality option accepted") }
            catch HostError.message { }
        }
        let board = NSPasteboard.withUniqueName(); defer { board.releaseGlobally() }
        let thirtyApp = AppDelegate(options: thirtyOptions, clipboard: board)
        let fullOptions = try Options.parse(args + ["--video-color", "444-atlas-v1"])
        let full = AppDelegate(options: fullOptions, clipboard: board)
        var fullSent: [[String: Any]] = []; full.wireSink = { fullSent.append($0) }
        full.receive(Incoming(object:["type":"hello","capabilities":["native.h264","video.color.444-atlas-v1"]],image:nil))
        precondition(fullSent.first?["color"] as? String == "444-atlas-v1")
        precondition(fullSent.first?["codec"] as? String == "h264")
        let aligned = AppDelegate(options: fullOptions, clipboard: board)
        var alignedSent: [[String: Any]] = []; aligned.wireSink = { alignedSent.append($0) }
        aligned.receive(Incoming(object:["type":"hello","capabilities":["native.h264","video.color.444-atlas-v1","video.color.444-atlas-v2"]],image:nil))
        precondition(alignedSent.first?["color"] as? String == "444-atlas-v2")
        var thirtySent: [[String: Any]] = []; thirtyApp.wireSink = { thirtySent.append($0) }
        thirtyApp.receive(Incoming(object:["type":"hello","capabilities":["native.h264","video.qos"],"video_max_fps":60],image:nil))
        precondition(thirtySent.first?["max_fps"] as? Int == 30, "the launch argument must reach video.enable")
        let app = AppDelegate(options: options, clipboard: board)
        var sent: [[String: Any]] = []
        app.wireSink = { sent.append($0) }
        app.receive(Incoming(object:["type":"hello", "capabilities":["native.h264", "display.scale"]], image:nil))
        precondition(sent.count == 2)
        precondition(sent[0]["type"] as? String == "video.enable" && sent[0]["codec"] as? String == "h264")
        precondition(sent[0]["refine_static"] == nil && sent[1]["type"] as? String == "session.resume")
        precondition(sent[1]["session_id"] as? String == "retained")
        let adaptive = AppDelegate(options: options, clipboard: board)
        var adaptiveSent: [[String: Any]] = []
        adaptive.wireSink = { adaptiveSent.append($0) }
        adaptive.receive(Incoming(object:["type":"hello", "capabilities":["native.h264", "video.qos"], "video_max_fps":75], image:nil))
        precondition(adaptiveSent[0]["max_fps"] as? Int == 60 && adaptive.videoFPS == 60
                     && adaptiveSent[0]["feedback"] as? Bool == true)
        adaptive.rateMenuItem = NSMenuItem()
        let thirty = NSMenuItem(), sixty = NSMenuItem(); thirty.tag = 30; sixty.tag = 60
        adaptive.rateChoices = [thirty, sixty]
        adaptive.selectVideoFPS(thirty)
        precondition(adaptiveSent.last?["type"] as? String == "video.configure" && adaptiveSent.last?["max_fps"] as? Int == 30)
        precondition(thirty.state == .on && sixty.state == .off)
        let selectedCount = adaptiveSent.count
        adaptive.screenParametersChanged(Notification(name:NSApplication.didChangeScreenParametersNotification))
        precondition(adaptive.videoFPS == 30 && adaptiveSent.count == selectedCount,
                     "screen changes must not override the selected frame rate")
        adaptive.selectVideoFPS(sixty)
        precondition(adaptiveSent.last?["max_fps"] as? Int == 60 && sixty.state == .on)
        adaptive.publisherMaxVideoFPS = 30; adaptive.refreshVideoFPS()
        precondition(adaptive.videoFPS == 30 && adaptive.requestedVideoFPS == 60)
        precondition(adaptive.rateMenuItem!.title.contains("远端上限 30"))
        adaptive.publisherMaxVideoFPS = 60; adaptive.refreshVideoFPS()
        precondition(adaptiveSent.last?["max_fps"] as? Int == 60)
        precondition(effectiveVideoFPS(requested:30,publisherMax:90) == 30)
        precondition(effectiveVideoFPS(requested:60,publisherMax:30) == 30)
        precondition(VideoActivity.select([(true, true), (false, false)]) == .foreground)
        precondition(VideoActivity.select([(true, false), (false, true)]) == .background)
        precondition(VideoActivity.select([(false, false)]) == .suspended)
        let managed = AppDelegate(options: thirtyOptions, clipboard: board)
        var managedSent: [[String: Any]] = []; managed.wireSink = { managedSent.append($0) }
        managed.receive(Incoming(object:["type":"hello", "capabilities":["native.h264", "video.qos", "video.activity"]], image:nil))
        for state in [VideoActivity.background, .suspended, .foreground] { managed.applyVideoActivity(state) }
        precondition(managedSent.filter { $0["type"] as? String == "video.activity" }.compactMap { $0["state"] as? String } == ["background", "suspended", "foreground"])
        precondition(managed.requestedVideoFPS == 30 && managed.configuredVideoFPS == 30, "activity must not overwrite the selected cap")
        let activityCount = managedSent.count; managed.applyVideoActivity(.foreground)
        precondition(managedSent.count == activityCount, "identical activity does not flood control")
        adaptive.applyVideoActivity(.background)
        precondition(adaptiveSent.last?["max_fps"] as? Int == 5)
        adaptive.applyVideoActivity(.suspended)
        precondition(adaptiveSent.last?["max_fps"] as? Int == 1)
        adaptive.applyVideoActivity(.foreground)
        precondition(adaptiveSent.last?["max_fps"] as? Int == 60)
        precondition(!adaptiveSent.contains { $0["type"] as? String == "video.activity" }, "legacy publishers only receive supported requests")
        adaptive.pendingVideoFeedbackID = 41
        adaptive.clipboardBusy = true
        adaptive.pump()
        precondition(adaptiveSent.last?["type"] as? String == "video.feedback" && adaptiveSent.last?["frame_id"] as? UInt64 == 41)
        let count = adaptiveSent.count
        adaptive.pendingVideoFeedbackID = 42
        adaptive.pump()
        precondition(adaptiveSent.count == count, "feedback should be rate limited")
        adaptive.lastVideoFeedback = ProcessInfo.processInfo.systemUptime - 0.1
        adaptive.pump()
        precondition(adaptiveSent.last?["frame_id"] as? UInt64 == 42, "the final still frame must be acknowledged")
        func report(_ id: UInt64) -> VideoFeedbackTiming {
            let now = DispatchTime.now().uptimeNanoseconds
            return VideoFeedbackTiming(frameID: id, received: now - 7_000_000, decoding: now - 6_000_000,
                decoded: now - 4_000_000, presented: now - 1_000_000)!
        }
        adaptive.pendingVideoFeedbackID = 43; adaptive.pendingVideoFeedbackTiming = report(43)
        adaptive.lastVideoFeedback = 0; adaptive.pump()
        precondition(adaptiveSent.last?["timing"] == nil, "legacy publishers must not receive new fields")
        adaptive.capabilities.insert("video.feedback.timing.v1")
        adaptive.pendingVideoFeedbackID = 44; adaptive.pendingVideoFeedbackTiming = report(44)
        adaptive.lastVideoFeedback = 0; adaptive.pump()
        let sentTiming = adaptiveSent.last?["timing"] as? [String: UInt64]
        precondition(sentTiming?["decode_us"] == 2000 && sentTiming?["present_us"] == 3000)
        adaptive.pendingVideoFeedbackID = 45; adaptive.pendingVideoFeedbackTiming = report(44)
        adaptive.lastVideoFeedback = 0; adaptive.pump()
        precondition(adaptiveSent.last?["timing"] == nil, "measurements must belong to the acknowledged frame")
        // A normal 60 fps stream must return every presentation credit. A
        // burst still coalesces to its newest frame, including the final still.
        let pacedCount = adaptiveSent.count
        let base = adaptive.lastVideoFeedback
        for i in 1...60 {
            adaptive.pendingVideoFeedbackID = UInt64(100 + i)
            adaptive.flushVideoFeedback(now: base + Double(i) / 60)
        }
        precondition(adaptiveSent.count == pacedCount + 60, "60 fps feedback must not wait for 50 ms batches")
        for i in 1...10 {
            adaptive.pendingVideoFeedbackID = UInt64(160 + i)
            adaptive.flushVideoFeedback(now: base + 1 + Double(i) / 10000)
        }
        precondition(adaptiveSent.count == pacedCount + 60, "burst feedback remains bounded")
        adaptive.flushVideoFeedback(now: base + 1.02)
        precondition(adaptiveSent.last?["frame_id"] as? UInt64 == 170)
        let context = CGContext(data: nil, width: 4, height: 4, bitsPerComponent: 8, bytesPerRow: 16,
                                space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
        let image = context.makeImage()!
        let host = HostedWindow(id: 9, width: 4, height: 4, revision: 1, scale: 1, popup: false,
                                disconnect: {}, transmit: { _ in })
        for _ in 0..<5 {
            host.present(image, mediaMode: "H.264 视频")
            Thread.sleep(forTimeInterval: 0.11)
        }
        precondition(host.window.subtitle.contains("fps"), "active frames should show a receive rate")
        adaptive.windows[host.id] = host
        adaptive.session = "retained"
        adaptive.lastVideoFeedback = 0
        let received = DispatchTime.now().uptimeNanoseconds - 3_000_000
        adaptive.receive(Incoming(object: ["type": "frame", "session_id": "retained", "seq": UInt64(1), "window_id": host.id,
            "geometry_revision": host.revision, "frame_id": UInt64(171),
            "received_ns": received, "decode_started_ns": received + 1_000_000,
            "decode_completed_ns": received + 2_000_000], image: image))
        let beforePresentation = adaptiveSent.count
        host.surface.frameWasPresented(host.surface.displayFrame!)
        precondition(adaptiveSent.count == beforePresentation + 1, "presentation must send feedback without waiting for pump")
        precondition(adaptiveSent.last?["frame_id"] as? UInt64 == 171)
        let presentedTiming = adaptiveSent.last?["timing"] as? [String: UInt64]
        precondition(presentedTiming?["queue_us"] == 1000 && presentedTiming?["decode_us"] == 1000,
            "local timestamps must survive receive/presentation into negotiated feedback")
        host.surface.frameWasPresented(host.surface.displayFrame!)
        precondition(adaptiveSent.count == beforePresentation + 1, "a redraw cannot return duplicate credit")
        Thread.sleep(forTimeInterval: 2.1)
        adaptive.pump()
        precondition(host.window.subtitle.contains("近 2 秒未收到新画面"), "an idle source must not retain an old fps")
        adaptive.windows.removeValue(forKey: host.id)
        host.closeLocally()
        let desktopApp = AppDelegate(options: options, clipboard: board)
        var desktopSent: [[String: Any]] = []
        desktopApp.wireSink = { desktopSent.append($0) }
        desktopApp.updateWindow(["window_id": UInt64(1), "width": 800, "height": 600,
            "geometry_revision": UInt64(1), "scale_factor": 1, "role": "desktop",
            "surface_type": "desktop", "title": "Windows display", "visible": true], create: true)
        let screen = desktopApp.windows[1]!
        precondition(screen.desktopMode && screen.fixedCanvas)
        screen.window.setContentSize(NSSize(width: 640, height: 480))
        screen.windowDidResize(Notification(name: NSWindow.didResizeNotification))
        screen.windowDidEndLiveResize(Notification(name: NSWindow.didEndLiveResizeNotification))
        RunLoop.main.run(until: Date().addingTimeInterval(0.3))
        precondition(!desktopSent.contains { $0["type"] as? String == "window.configure" }, "desktop resize stays local")
        screen.closeLocally(); desktopApp.windows.removeAll(); desktopApp.stopping = true
        var detachCount = 0
        var closeSent: [[String: Any]] = []
        let detachOnly = HostedWindow(id: 2, width: 800, height: 600, revision: 1, scale: 1, popup: false,
            disconnect: { detachCount += 1 }, transmit: { closeSent.append($0) })
        detachOnly.desktopMode = true
        precondition(!detachOnly.windowShouldClose(detachOnly.window) && detachCount == 1)
        precondition(!closeSent.contains { $0["type"] as? String == "window.close.request" })
        detachOnly.closeLocally()
        print("PASS: Windows desktop metadata, fixed source geometry and detach-only close")
        print("PASS: H.264 negotiation, 30/60 selection, live configure, publisher limits, idle feedback and current receive-rate display")
    }
}
