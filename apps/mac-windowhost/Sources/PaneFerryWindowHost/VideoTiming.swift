import Foundation

/// Per-frame elapsed intervals on this receiver's monotonic clock. Only sent
/// when the publisher advertises support; no absolute timestamps or content.
struct VideoFeedbackTiming {
    let frameID: UInt64
    let received: UInt64, decoding: UInt64, decoded: UInt64, presented: UInt64
    init?(frameID: UInt64, received: UInt64?, decoding: UInt64?, decoded: UInt64?, presented: UInt64) {
        guard let received = received, let decoding = decoding, let decoded = decoded,
              received <= decoding, decoding <= decoded, decoded <= presented else { return nil }
        self.frameID = frameID; self.received = received
        self.decoding = decoding; self.decoded = decoded; self.presented = presented
    }
    func wire(at now: UInt64) -> [String: UInt64]? {
        guard now >= presented else { return nil }
        let queue = (decoding - received) / 1000
        let decode = (decoded - decoding) / 1000
        let present = (presented - decoded) / 1000
        let hold = (now - presented) / 1000
        guard queue <= 5_000_000, decode <= 5_000_000, present <= 5_000_000, hold <= 5_000_000 else { return nil }
        return ["queue_us": queue, "decode_us": decode, "present_us": present, "feedback_wait_us": hold]
    }
}

/// Local monotonic intervals only. No content, titles, keys or network identity.
enum VideoTiming {
    static let enabled = ProcessInfo.processInfo.environment["PANEFERRY_VIDEO_TIMING"] == "1"
    private static let lock = NSLock()
    private static var samples: [String: [UInt64]] = [:]
    private static var started = DispatchTime.now().uptimeNanoseconds
    static func record(_ stage: String, since: UInt64) {
        guard enabled else { return }
        let now = DispatchTime.now().uptimeNanoseconds
        guard now >= since else { return }
        lock.lock(); defer { lock.unlock() }
        if samples[stage, default: []].count < 2048 { samples[stage, default: []].append((now - since) / 1000) }
        guard now - started >= 2_000_000_000 else { return }
        for (name, values) in samples where !values.isEmpty {
            let sorted = values.sorted(), last = values.count - 1
            let line = "PF_TIMING stage=\(name) interval_ms=\((now-started)/1_000_000) count=\(values.count) p50_us=\(sorted[values.count/2]) p95_us=\(sorted[last*95/100]) p99_us=\(sorted[last*99/100])\n"
            if let bytes = line.data(using: .utf8) { FileHandle.standardError.write(bytes) }
        }
        samples.removeAll(keepingCapacity: true); started = now
    }
}
