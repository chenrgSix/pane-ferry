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
        let board = NSPasteboard.withUniqueName(); defer { board.releaseGlobally() }
        let app = AppDelegate(options: options, clipboard: board)
        var sent: [[String: Any]] = []
        app.wireSink = { sent.append($0) }
        app.receive(Incoming(object:["type":"hello", "capabilities":["native.h264", "display.scale"]], image:nil))
        precondition(sent.count == 2)
        precondition(sent[0]["type"] as? String == "video.enable" && sent[0]["codec"] as? String == "h264")
        precondition(sent[0]["refine_static"] == nil && sent[1]["type"] as? String == "session.resume")
        precondition(sent[1]["session_id"] as? String == "retained")
        print("PASS: H.264 is mandatory and negotiated before session attachment; PNG mode and refinement are absent")
    }
}
