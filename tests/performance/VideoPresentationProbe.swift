import CoreVideo
import IOSurface

// Probe-only: timestamp when AVFoundation exposes the submitted surface as the
// displayed buffer. Pixel inspection happens afterward and never gates product
// rendering. This includes renderer scheduling, but not physical panel scanout.
extension Probe {
    static var videoSelections: [ObjectIdentifier: [String: Any]] = [:]
    static func selected(_ frame: VideoFrame, object: [String: Any]) {
        if videoSelections.count > 64 { videoSelections.removeAll() }
        var value = object; value["selected"] = now()
        videoSelections[ObjectIdentifier(frame)] = value
    }
    static func presented(_ frame: VideoFrame, surface: SurfaceView) {
        let deadline = now() + 1_000_000_000
        func inspect() {
            guard surface.displayFrame === frame else { return }
            if let buffer = frame.pixelBuffer {
                guard let displayed = surface.displayedPixelBuffer() else {
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
            let completed = now()
            guard let value = videoSelections[ObjectIdentifier(frame)], let image = frame.image else { return }
            lock.lock(); frames[ObjectIdentifier(image)] = value; lock.unlock()
            drew(image, timestamp: completed)
        }
        inspect()
    }
}
