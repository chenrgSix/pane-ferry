import AVFoundation
import CoreImage
import CoreVideo

enum VideoDecodeError: Error { case invalidFrame, decoder(OSStatus) }

/// A decoded surface stays in IOSurface/CoreVideo through presentation. The
/// CGImage accessor is only for PNG fallback, snapshots and pixel-based tests.
final class VideoFrame {
    let width: Int, height: Int
    let pixelBuffer: CVPixelBuffer?
    let sample: CMSampleBuffer?
    private let lock = NSLock()
    private var bitmap: CGImage?
    private static let context = CIContext(options: [.cacheIntermediates: false])
    init(image: CGImage) {
        width = image.width; height = image.height; bitmap = image
        pixelBuffer = nil; sample = nil
    }
    init(buffer: CVPixelBuffer, width: Int, height: Int) throws {
        self.width = width; self.height = height; pixelBuffer = buffer
        // The encoder pads odd dimensions at the right/bottom. Preserve the
        // logical aperture without cropping or copying the decoded pixels.
        let aperture: [CFString: Any] = [
            kCVImageBufferCleanApertureWidthKey: width,
            kCVImageBufferCleanApertureHeightKey: height,
            kCVImageBufferCleanApertureHorizontalOffsetKey: Double(width - CVPixelBufferGetWidth(buffer)) / 2,
            kCVImageBufferCleanApertureVerticalOffsetKey: Double(height - CVPixelBufferGetHeight(buffer)) / 2]
        CVBufferSetAttachment(buffer, kCVImageBufferCleanApertureKey, aperture as CFDictionary, .shouldPropagate)
        var format: CMVideoFormatDescription?
        var status = CMVideoFormatDescriptionCreateForImageBuffer(allocator: kCFAllocatorDefault, imageBuffer: buffer, formatDescriptionOut: &format)
        guard status == noErr, let format = format else { throw VideoDecodeError.decoder(status) }
        var timing = CMSampleTimingInfo(duration: .invalid, presentationTimeStamp: .zero, decodeTimeStamp: .invalid)
        var sample: CMSampleBuffer?
        status = CMSampleBufferCreateReadyWithImageBuffer(allocator: kCFAllocatorDefault, imageBuffer: buffer,
            formatDescription: format, sampleTiming: &timing, sampleBufferOut: &sample)
        guard status == noErr, let sample = sample else { throw VideoDecodeError.decoder(status) }
        if let attachments = CMSampleBufferGetSampleAttachmentsArray(sample, createIfNecessary: true) {
            let values = unsafeBitCast(CFArrayGetValueAtIndex(attachments, 0), to: CFMutableDictionary.self)
            CFDictionarySetValue(values, Unmanaged.passUnretained(kCMSampleAttachmentKey_DisplayImmediately).toOpaque(),
                Unmanaged.passUnretained(kCFBooleanTrue).toOpaque())
        }
        self.sample = sample
    }
    var image: CGImage? {
        lock.lock(); defer { lock.unlock() }
        if let bitmap = bitmap { return bitmap }
        guard let buffer = pixelBuffer,
              let full = Self.context.createCGImage(CIImage(cvPixelBuffer: buffer),
                from: CGRect(x: 0, y: 0, width: CVPixelBufferGetWidth(buffer), height: CVPixelBufferGetHeight(buffer))) else { return nil }
        bitmap = full.cropping(to: CGRect(x: 0, y: 0, width: width, height: height))
        return bitmap
    }
}
