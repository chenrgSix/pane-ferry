import Foundation

// Optional-runtime integration fixture. All inputs are owned test packets.
@main struct WebRTCMediaSmoke {
    static func main() throws {
        let outputLock = NSLock()
        func emit(_ object: [String: Any]) {
            let bytes = try! JSONSerialization.data(withJSONObject: object)
            outputLock.lock(); defer { outputLock.unlock() }
            FileHandle.standardOutput.write(bytes + Data([10]))
        }
        let peer = try WebRTCPeer(signal: emit, frame: { incoming in
            emit(["type": "test.frame", "frame_id": incoming.object["frame_id"]!,
                  "pixel_buffer": incoming.displayFrame?.pixelBuffer != nil,
                  "hardware_decoder": incoming.object["hardware_decoder"] ?? false])
        }, failure: { emit(["type": "test.failed"]) })
        defer { peer.close() }
        while let line = readLine() {
            let object = try JSONSerialization.jsonObject(with: Data(line.utf8)) as! [String: Any]
            switch object["type"] as? String {
            case "webrtc.offer": peer.description(object["sdp"] as! String)
            case "webrtc.candidate": peer.remoteCandidate(object["candidate"] as! String, mid: object["mid"] as! String)
            case "test.close": return
            default: peer.noteControl(object)
            }
        }
    }
}
