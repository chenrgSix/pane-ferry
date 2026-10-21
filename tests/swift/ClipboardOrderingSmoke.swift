import AppKit
@main
struct ClipboardOrderingSmoke {
    static func main() {
        _ = NSApplication.shared
        let board = NSPasteboard.withUniqueName(); defer { board.releaseGlobally() }
        let app = AppDelegate(options: Options(host: "invalid.example", user: "fixture", port: 22, app: nil, session: "fixture"), clipboard: board)
        app.capabilities = ["clipboard.copy", "clipboard.cut", "clipboard.paste", "clipboard.bundle"]
        app.session = "fixture"
        let host = HostedWindow(id: 42, width: 640, height: 480, revision: 1, scale: 1, popup: false, disconnect: {}) { _ in }
        app.windows[42] = host
        var sent: [[String: Any]] = []; app.wireSink = { sent.append($0) }
        func set(_ text: String) { board.clearContents(); board.setString(text, forType: .string) }
        func reply(_ id: String, _ text: String, _ sequence: Int) {
            app.receive(Incoming(object: ["type":"clipboard.data", "request_id":id, "window_id":42, "session_id":"fixture", "seq":sequence, "text":text], image:nil))
        }
        set("old")
        app.send(["type":"clipboard.cut", "window_id":UInt32(42), "request_id":"cut"])
        app.send(["type":"controller.paste", "window_id":UInt32(42)])
        precondition(sent.count == 1 && sent[0]["type"] as? String == "clipboard.copy.bundle" && app.clipboardBusy)
        reply("wrong", "unsolicited", 1)
        precondition(board.string(forType:.string) == "old" && sent.count == 1)
        reply("cut", "new selected text", 2)
        precondition(!app.clipboardBusy && board.string(forType:.string) == "new selected text")
        precondition(sent.count == 2 && sent[1]["type"] as? String == "clipboard.paste" && sent[1]["text"] as? String == "new selected text", "fast cut then paste must use the newly received payload")
        app.send(["type":"clipboard.copy", "window_id":UInt32(42), "request_id":"failed"])
        app.send(["type":"controller.paste", "window_id":UInt32(42)])
        let beforeFailure = sent.count
        app.receive(Incoming(object:["type":"error", "code":"clipboard_copy_failed"], image:nil))
        precondition(sent.count == beforeFailure && !app.clipboardBusy, "failed copy must cancel its queued paste instead of pasting stale contents")
        app.send(["type":"clipboard.copy", "window_id":UInt32(42), "request_id":"late"])
        set("copied locally later")
        reply("late", "older remote reply", 3)
        precondition(board.string(forType:.string) == "copied locally later", "a late remote reply cannot overwrite a newer local clipboard change")
        host.closeLocally(); app.clipboardTimer?.invalidate()
        print("PASS: actual AppDelegate cut→paste ordering, unsolicited response rejection, failed-copy paste cancellation and newer local clipboard preservation")
    }
}
