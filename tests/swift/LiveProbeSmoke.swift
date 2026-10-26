import AppKit
import CoreVideo

@main struct LiveProbeSmoke {
    static func main() throws {
        for format in [kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange, kCVPixelFormatType_420YpCbCr8BiPlanarFullRange] {
            var storage: CVPixelBuffer?
            precondition(CVPixelBufferCreate(kCFAllocatorDefault, 258, 50, format,
                [kCVPixelBufferIOSurfacePropertiesKey: [:]] as CFDictionary, &storage) == kCVReturnSuccess)
            let buffer = storage!
            let frame = try VideoFrame(buffer: buffer, width: 257, height: 49)
            for count in 0..<256 {
                CVPixelBufferLockBaseAddress(buffer, [])
                let stride = CVPixelBufferGetBytesPerRowOfPlane(buffer, 0)
                let y = CVPixelBufferGetBaseAddressOfPlane(buffer, 0)!.assumingMemoryBound(to: UInt8.self)
                memset(y, 96, stride * 50)
                for bit in 0..<8 {
                    let white = count & (1 << bit) != 0
                    y[16 * stride + 16 + bit * 32] = white ? 235 : 16
                    y[33 * stride + 16 + bit * 32] = white ? 16 : 235
                }
                CVPixelBufferUnlockBaseAddress(buffer, [])
                precondition(Probe.videoMarker(frame) == count, "NV12 marker must use the top row and actual plane stride")
            }
            let small = try VideoFrame(buffer: buffer, width: 128, height: 49)
            precondition(Probe.videoMarker(small) == nil)
        }
        let context = CGContext(data: nil, width: 256, height: 50, bitsPerComponent: 8, bytesPerRow: 256 * 4,
            space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
        context.setFillColor(NSColor.black.cgColor); context.fill(CGRect(x: 0, y: 0, width: 256, height: 50))
        for bit in 0..<8 where 85 & (1 << bit) != 0 {
            context.setFillColor(NSColor.white.cgColor)
            context.fill(CGRect(x: bit * 32, y: 0, width: 32, height: 50))
        }
        let bitmap = VideoFrame(image: context.makeImage()!)
        precondition(Probe.videoMarker(bitmap) == 85, "bitmap fallback must recognize the same marker")
        let other = VideoFrame(image: context.makeImage()!)
        Probe.selected(bitmap, object: ["probe_label": "first"])
        Probe.videoStage(bitmap, "enqueue_begin")
        Probe.selected(other, object: ["probe_label": "second"])
        Probe.videoStage(other, "enqueue_end")
        let firstStages = Probe.videoSelections[ObjectIdentifier(bitmap)]!["mac_stages"] as! [String: UInt64]
        let otherStages = Probe.videoSelections[ObjectIdentifier(other)]!["mac_stages"] as! [String: UInt64]
        precondition(firstStages["enqueue_begin"] != nil && firstStages["enqueue_end"] == nil)
        precondition(otherStages["enqueue_begin"] == nil && otherStages["enqueue_end"] != nil,
                     "asynchronous presentation timestamps must stay with their exact frame")
        Probe.current = ["count": 7, "start": UInt64(100), "warmup": false]
        Probe.matched(6, object: ["codec": "h264"], completed: 150)
        precondition(Probe.rows.isEmpty && Probe.current != nil, "a stale displayed frame must not complete an input")
        Probe.matched(7, object: ["codec": "h264"], completed: 200)
        precondition(Probe.rows.count == 1 && Probe.current == nil)
        precondition(Probe.rows[0]["draw_end"] as? UInt64 == 200, "pixel matching must retain the earlier display timestamp")
        print("PASS: all 256 NV12 markers in both ranges, padded stride, top-row orientation, undersized rejection, bitmap fallback, stale frame isolation, display timestamp and per-frame presentation stages")
    }
}
