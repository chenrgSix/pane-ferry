import Foundation
import CoreGraphics

@main struct AsyncVideoDecodeSmoke {
    static func main() throws {
        let entered = DispatchSemaphore(value: 0), release = DispatchSemaphore(value: 0)
        let other = DispatchSemaphore(value: 0), finished = DispatchSemaphore(value: 0)
        let lock = NSLock()
        var delivered: [UInt64] = []
        let queue = VideoDecodeQueue { job, result, _ in
            if case .failure = result { preconditionFailure("unexpected decode error") }
            lock.lock(); delivered.append(job.generation); lock.unlock()
            if job.window == 2 { other.signal() }
            if job.generation == 3 { finished.signal() }
        }
        queue.decodeOverride = { job in
            if job.generation == 1 { entered.signal(); _ = release.wait(timeout: .now() + 5) }
            return nil
        }
        func job(_ window: UInt32, _ generation: UInt64, bytes: Int = 1) -> VideoDecodeQueue.Job {
            .init(window: window, generation: generation, stream: 1, width: 2, height: 2, encodedWidth: 2, encodedHeight: 2,
                  data: Data(repeating: 0, count: bytes), object: [:])
        }
        try queue.submit(job(1, 1))
        precondition(entered.wait(timeout: .now() + 2) == .success)
        // Another stream must decode while stream 1's driver is blocked.
        try queue.submit(job(2, 2))
        precondition(other.wait(timeout: .now() + 2) == .success)
        queue.invalidate(1)
        try queue.submit(job(1, 3))
        precondition(finished.wait(timeout: .now() + 2) == .success)
        release.signal()
        Thread.sleep(forTimeInterval: 0.1)
        lock.lock(); let result = delivered; lock.unlock()
        precondition(result == [2, 3], "retired in-flight work cannot publish into a reused ID")
        queue.close()

        let hold = DispatchSemaphore(value: 0), started = DispatchSemaphore(value: 0)
        let bounded = VideoDecodeQueue { _, _, _ in }
        bounded.decodeOverride = { _ in started.signal(); _ = hold.wait(timeout: .now() + 5); return nil }
        try bounded.submit(job(1, 1, bytes: 12 * 1024 * 1024))
        precondition(started.wait(timeout: .now() + 2) == .success)
        try bounded.submit(job(1, 1, bytes: 12 * 1024 * 1024))
        do { try bounded.submit(job(1, 1)); preconditionFailure("unbounded compressed queue") }
        catch VideoDecodeError.invalidFrame { }
        bounded.close(); hold.signal()
        print("PASS: stalled decode isolation, retired/reused stream cancellation and in-flight compressed-byte budget")
    }
}
