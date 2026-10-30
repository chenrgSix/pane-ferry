import Foundation

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
