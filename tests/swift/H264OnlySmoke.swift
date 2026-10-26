import AppKit

@main
struct H264OnlySmoke {
    static func main() throws {
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
        Thread.sleep(forTimeInterval: 2.1)
        adaptive.pump()
        precondition(host.window.subtitle.contains("近 2 秒未收到新画面"), "an idle source must not retain an old fps")
        adaptive.windows.removeValue(forKey: host.id)
        host.closeLocally()
        print("PASS: H.264 negotiation, 30/60 selection, live configure, publisher limits, idle feedback and current receive-rate display")
    }
}
