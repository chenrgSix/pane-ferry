import Foundation
import CoreGraphics

/// A bounded fair scheduler: one ordered decoder per stream, at most two decode
/// jobs executing. The wire reader never waits for VideoToolbox or CoreImage.
final class VideoDecodeQueue {
    struct Job {
        let window: UInt32
        let generation: UInt64
        let stream: UInt64
        let width: Int, height: Int, encodedWidth: Int, encodedHeight: Int
        let data: Data
        let object: [String: Any]
    }
    private final class Lane {
        var jobs: [Job] = []
        var busy = false
        var retired = false
        var generation: UInt64?
        var decoder: H264Decoder?
    }
    private let lock = NSLock()
    private let workers: OperationQueue = {
        let queue = OperationQueue()
        queue.name = "paneferry.video.decode"
        queue.maxConcurrentOperationCount = 2
        queue.qualityOfService = .userInteractive
        return queue
    }()
    private var lanes: [UInt32: Lane] = [:]
    private var bytes = 0, count = 0, outstanding = 0
    private var closed = false
    private let complete: (Job, Result<VideoFrame?, Error>, Bool) -> Void
    // Injection is restricted to native tests; production always uses H264Decoder.
    var decodeOverride: ((Job) throws -> CGImage?)?
    init(complete: @escaping (Job, Result<VideoFrame?, Error>, Bool) -> Void) { self.complete = complete }

    func submit(_ job: Job) throws {
        lock.lock(); defer { lock.unlock() }
        guard !closed else { return }
        guard count < 64, bytes + job.data.count <= 24 * 1024 * 1024 else { throw VideoDecodeError.invalidFrame }
        let lane: Lane
        if let existing = lanes[job.window] { lane = existing }
        else {
            guard lanes.count < 64, outstanding < 64 else { throw VideoDecodeError.invalidFrame }
            lane = Lane(); lanes[job.window] = lane
        }
        lane.jobs.append(job); bytes += job.data.count; count += 1
        schedule(lane, window: job.window)
    }
    private func schedule(_ lane: Lane, window: UInt32) {
        guard !lane.busy, !lane.jobs.isEmpty, !lane.retired, !closed else { return }
        lane.busy = true; outstanding += 1
        workers.addOperation { [weak self, lane] in self?.run(lane, window: window) }
    }
    private func run(_ lane: Lane, window: UInt32) {
        lock.lock()
        guard !closed, !lane.retired, !lane.jobs.isEmpty else {
            lane.busy = false; outstanding -= 1; lock.unlock(); return
        }
        let job = lane.jobs.removeFirst()
        // Include the currently decoding packet in the byte/count budget.
        lock.unlock()
        let decodingStarted = DispatchTime.now().uptimeNanoseconds
        // This timestamp is inserted locally by Transport, not read from the wire.
        if let received = job.object["received_ns"] as? UInt64 { VideoTiming.record("mac.decode_queue", since: received) }
        let result: Result<VideoFrame?, Error>
        do {
            if let decode = decodeOverride { result = .success(try decode(job).map { VideoFrame(image: $0) }) }
            else {
                if lane.generation != job.generation { lane.decoder = H264Decoder(); lane.generation = job.generation }
                guard let color = VideoColor(rawValue: job.object["color"] as? String ?? "420") else { throw VideoDecodeError.invalidFrame }
                result = .success(try lane.decoder!.decodeFrame(job.data, streamID: job.stream,
                    width: job.width, height: job.height, encodedWidth: job.encodedWidth, encodedHeight: job.encodedHeight, colorMode: color))
            }
        } catch { result = .failure(error) }
        VideoTiming.record("mac.decode", since: decodingStarted)
        lock.lock()
        bytes -= job.data.count; count -= 1
        let deliver = !closed && !lane.retired
        lock.unlock()
        if deliver { complete(job, result, lane.decoder?.hardwareAccelerated ?? false) }
        lock.lock()
        lane.busy = false; outstanding -= 1
        schedule(lane, window: window)
        lock.unlock()
    }
    func invalidate(_ window: UInt32) {
        lock.lock(); defer { lock.unlock() }
        guard let lane = lanes.removeValue(forKey: window) else { return }
        lane.retired = true
        for job in lane.jobs { bytes -= job.data.count; count -= 1 }
        lane.jobs.removeAll()
    }
    func reset() {
        lock.lock(); defer { lock.unlock() }
        for lane in lanes.values {
            lane.retired = true
            for job in lane.jobs { bytes -= job.data.count; count -= 1 }
            lane.jobs.removeAll()
        }
        lanes.removeAll()
    }
    func close() {
        lock.lock(); closed = true; lock.unlock()
        reset()
    }
}
