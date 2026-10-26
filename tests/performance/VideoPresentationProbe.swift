import CoreVideo
import IOSurface

// Probe-only: timestamp when AVFoundation exposes the submitted surface as the
// displayed buffer. Pixel inspection happens afterward and never gates product
// rendering. This includes renderer scheduling, but not physical panel scanout.
extension Probe {
    static var selectedVideoCount = 0, presentedVideoCount = 0, matchedVideoCount = 0
    static var videoSelections: [ObjectIdentifier: [String: Any]] = [:]
    static func selected(_ frame: VideoFrame, object: [String: Any]) {
        selectedVideoCount += 1
        lock.lock(); defer { lock.unlock() }
        if videoSelections.count > 64 { videoSelections.removeAll() }
        var value = object; value["selected"] = now()
        videoSelections[ObjectIdentifier(frame)] = value
    }
    static func videoStage(_ frame: VideoFrame, _ stage: String) {
        let timestamp = now()
        lock.lock(); defer { lock.unlock() }
        guard var value = videoSelections[ObjectIdentifier(frame)] else { return }
        var stages = value["mac_stages"] as? [String: UInt64] ?? [:]
        stages[stage] = timestamp
        value["mac_stages"] = stages
        videoSelections[ObjectIdentifier(frame)] = value
    }
    static func presented(_ frame: VideoFrame, surface: SurfaceView) {
        presentedVideoCount += 1
        videoStage(frame, "probe_started")
        let deadline = now() + 1_000_000_000
        var queries = 0
        var queryNanoseconds: UInt64 = 0
        func inspect() {
            guard surface.displayFrame === frame else { return }
            if let buffer = frame.pixelBuffer {
                let queryStart = now()
                let displayedBuffer = surface.displayedPixelBuffer()
                queryNanoseconds += now() - queryStart
                queries += 1
                guard let displayed = displayedBuffer else {
                    if now() < deadline { DispatchQueue.main.asyncAfter(deadline: .now() + 0.001, execute: inspect) }
                    return
                }
                let sameSurface = CVPixelBufferGetIOSurface(displayed).flatMap { one in
                    CVPixelBufferGetIOSurface(buffer).map { two in IOSurfaceGetID(one.takeUnretainedValue()) == IOSurfaceGetID(two.takeUnretainedValue()) }
                } ?? false
                guard displayed === buffer || sameSurface else {
                    if now() < deadline { DispatchQueue.main.asyncAfter(deadline: .now() + 0.001, execute: inspect) }
                    return
                }
            }
            matchedVideoCount += 1
            let completed = now()
            lock.lock()
            let stored = videoSelections[ObjectIdentifier(frame)]
            lock.unlock()
            guard var value = stored else { return }
            value["mac_query_count"] = queries
            value["mac_query_total_ns"] = queryNanoseconds
            value["mac_renderer_match_ns"] = completed
            observedVideo(frame, object: value, completed: completed)
        }
        inspect()
    }
}
