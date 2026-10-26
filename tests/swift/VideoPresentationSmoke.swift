import AppKit
import AVFoundation

@main struct VideoPresentationSmoke {
    static func main() throws {
        let app = NSApplication.shared; app.setActivationPolicy(.regular)
        var buffer: CVPixelBuffer?
        precondition(CVPixelBufferCreate(kCFAllocatorDefault, 66, 50, kCVPixelFormatType_32BGRA,
            [kCVPixelBufferIOSurfacePropertiesKey: [:]] as CFDictionary, &buffer) == kCVReturnSuccess)
        let pixels = buffer!
        CVPixelBufferLockBaseAddress(pixels, [])
        memset(CVPixelBufferGetBaseAddress(pixels), 255, CVPixelBufferGetDataSize(pixels))
        CVPixelBufferUnlockBaseAddress(pixels, [])
        let frame = try VideoFrame(buffer: pixels, width: 65, height: 49)
        let format = CMSampleBufferGetFormatDescription(frame.sample!)!
        let aperture = CMVideoFormatDescriptionGetCleanAperture(format, originIsAtTopLeft: true)
        precondition(aperture == CGRect(x: 0, y: 0, width: 65, height: 49), "odd padding must be excluded at right/bottom: \(aperture)")
        precondition(CMSampleBufferGetImageBuffer(frame.sample!) === pixels, "display must retain the decoded surface without copying")
        let attachments = CMSampleBufferGetSampleAttachmentsArray(frame.sample!, createIfNecessary: false)! as NSArray
        precondition((attachments[0] as! NSDictionary)[kCMSampleAttachmentKey_DisplayImmediately] as? Bool == true)
        let surface = SurfaceView(frame: NSRect(x: 0, y: 0, width: 520, height: 392))
        surface.remoteWidth = 65; surface.remoteHeight = 49
        let window = NSWindow(contentRect: surface.bounds, styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
        window.contentView = surface; window.makeKeyAndOrderFront(nil); app.activate(ignoringOtherApps: true)
        surface.displayFrame = frame
        let deadline = Date().addingTimeInterval(5)
        while surface.displayedPixelBuffer() == nil && Date() < deadline { RunLoop.current.run(until: Date().addingTimeInterval(0.01)) }
        precondition(surface.displayedPixelBuffer() != nil, "AVFoundation never displayed the surface")
        precondition(surface.displayFrame === frame)
        precondition(frame.image?.width == 65 && frame.image?.height == 49, "diagnostic bitmap must retain logical dimensions")
        let video = surface.subviews.compactMap { $0.layer?.sublayers?.first { $0 is AVSampleBufferDisplayLayer } as? AVSampleBufferDisplayLayer }.first!
        func assertVideoPlacement() {
            CATransaction.flush()
            // The model frame alone misses implicit zoom animations. Inspect
            // the layer that Core Animation is actually presenting as well.
            let expected = video.superlayer!.bounds
            let displayed = video.presentation()!.frame
            precondition(abs(displayed.minX - expected.minX) < 0.01 && abs(displayed.minY - expected.minY) < 0.01 &&
                         abs(displayed.width - expected.width) < 0.01 && abs(displayed.height - expected.height) < 0.01,
                         "video must not animate away from its viewport: \(displayed) != \(expected)")
        }
        window.setContentSize(NSSize(width: 600, height: 300)); surface.layoutSubtreeIfNeeded()
        precondition(abs(surface.imageRect.width / surface.imageRect.height - 65.0 / 49) < 0.001)
        assertVideoPlacement()
        // A pending layout followed by another decoded frame uses the same
        // immediate geometry path, even before AppKit's next layout pass.
        window.setContentSize(NSSize(width: 500, height: 380))
        surface.displayFrame = try VideoFrame(buffer: pixels, width: 65, height: 49)
        assertVideoPlacement()
        let stableBounds = surface.bounds
        for _ in 0..<3 {
            app.hide(nil)
            RunLoop.current.run(until: Date().addingTimeInterval(0.02))
            app.unhideWithoutActivation(); app.activate(ignoringOtherApps: true); window.makeKeyAndOrderFront(nil)
            surface.needsLayout = true; surface.layoutSubtreeIfNeeded()
            precondition(surface.bounds == stableBounds, "activation must preserve the content size")
            assertVideoPlacement()
        }
        let bitmap = frame.image!
        surface.image = bitmap
        precondition(surface.displayFrame?.pixelBuffer == nil, "PNG fallback must hide/flush the video layer")
        surface.setMarkedText("窗渡", selectedRange: NSRange(location: 2, length: 0), replacementRange: NSRange(location: NSNotFound, length: 0))
        precondition(surface.hasMarkedText())
        surface.unmarkText(); window.orderOut(nil)
        print("PASS: retained CoreVideo surface actually displayed, odd clean aperture, immediate display, aspect ratio, immediate video geometry, activation, bitmap fallback and IME overlay")
    }
}
