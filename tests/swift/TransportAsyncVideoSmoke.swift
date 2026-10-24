import AppKit

@main struct TransportAsyncVideoSmoke {
    static func main() throws {
        let directory = URL(fileURLWithPath: CommandLine.arguments[1])
        let entered = DispatchSemaphore(value: 0), release = DispatchSemaphore(value: 0)
        let host = RWPTransport(decode: { job in
            if job.data.first == 0 {
                try Data().write(to: directory.appendingPathComponent("decoding"))
                entered.signal(); _ = release.wait(timeout: .now() + 5)
            }
            return CGContext(data: nil, width: 2, height: 2, bitsPerComponent: 8, bytesPerRow: 8,
                             space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue)!.makeImage()
        })
        let options = Options(app: nil, session: "fixture", native: NativeRoute(
            peer: "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", state: directory.path,
            helper: directory.appendingPathComponent("bridge").path))
        try host.start(options)
        defer { release.signal(); host.disconnect() }
        precondition(entered.wait(timeout: .now() + 3) == .success)
        var events: [Incoming] = []
        let deadline = Date().addingTimeInterval(2)
        while Date() < deadline {
            let (next, failure) = host.drain(); precondition(failure == nil)
            events.append(contentsOf: next)
            if events.contains(where: { $0.object["type"] as? String == "frame" && $0.object["frame_id"] as? Int == 2 }) { break }
            Thread.sleep(forTimeInterval: 0.01)
        }
        precondition(events.contains { $0.object["type"] as? String == "clipboard.data" }, "control must pass while decoder 1 is blocked")
        precondition(events.contains { $0.object["frame_id"] as? Int == 2 && $0.image != nil }, "recreated window decodes independently")
        release.signal(); Thread.sleep(forTimeInterval: 0.1)
        events.append(contentsOf: host.drain().0)
        precondition(!events.contains { $0.object["frame_id"] as? Int == 1 }, "stale completion cannot resurrect destroyed window")
        print("PASS: real bridge reader delivers control during blocked decode and rejects destroyed/reused-window completion")
    }
}
