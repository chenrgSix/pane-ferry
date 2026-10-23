import AppKit

@main
struct H264OnlySmoke {
    static func main() throws {
        _ = NSApplication.shared
        let args = ["--native-peer", "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", "--native-state", "/tmp/fixture",
                    "--native-helper", "/tmp/fixture/bridge", "--session", "retained"]
        let options = try Options.parse(args)
        for deprecated in ["png", "h264"] {
            do { _ = try Options.parse(args + ["--quality", deprecated]); preconditionFailure("obsolete quality option accepted") }
            catch HostError.message { }
        }
        let savedRate = UserDefaults.standard.object(forKey: "videoMaxFPS")
        UserDefaults.standard.set(60, forKey: "videoMaxFPS")
        defer {
            if let savedRate { UserDefaults.standard.set(savedRate, forKey: "videoMaxFPS") }
            else { UserDefaults.standard.removeObject(forKey: "videoMaxFPS") }
        }
        let board = NSPasteboard.withUniqueName(); defer { board.releaseGlobally() }
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
        adaptive.receive(Incoming(object:["type":"hello", "capabilities":["native.h264", "video.qos"]], image:nil))
        precondition(adaptiveSent[0]["max_fps"] as? Int == 60 && adaptiveSent[0]["feedback"] as? Bool == true)
        adaptive.setVideoFPS(90)
        precondition(adaptiveSent.last?["type"] as? String == "video.configure" && adaptiveSent.last?["max_fps"] as? Int == 90)
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
        print("PASS: H.264-only legacy negotiation, default 60/optional 90 fps, and idle-frame feedback")
    }
}
