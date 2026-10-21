import Foundation
import CoreGraphics
import CoreImage
import CoreMedia
import VideoToolbox

enum VideoDecodeError: Error { case invalidFrame, decoder(OSStatus) }

/// One decoder per window. All access units are decoded in order on the SSH reader;
/// only decoded display images may be coalesced. Dropping a P-frame would corrupt later frames.
final class H264Decoder {
    private(set) var hardwareAccelerated = false
    private var session: VTDecompressionSession?
    private var format: CMVideoFormatDescription?
    private var stream: UInt64?
    private var sps = Data(), pps = Data()
    private let context = CIContext(options: [.cacheIntermediates: false])
    private let resultLock = NSLock()
    private var output: CVImageBuffer?

    deinit { reset() }
    private func reset() {
        if let session = session { VTDecompressionSessionInvalidate(session) }
        session = nil; format = nil; sps.removeAll(); pps.removeAll(); hardwareAccelerated = false
    }
    static func nalUnits(_ data: Data) throws -> [Data] {
        let bytes = [UInt8](data)
        guard bytes.count >= 4 else { throw VideoDecodeError.invalidFrame }
        var starts: [(Int, Int)] = []
        var index = 0
        while index + 2 < bytes.count {
            if bytes[index] == 0 && bytes[index + 1] == 0 {
                if bytes[index + 2] == 1 { starts.append((index, 3)); index += 3; continue }
                if index + 3 < bytes.count && bytes[index + 2] == 0 && bytes[index + 3] == 1 {
                    starts.append((index, 4)); index += 4; continue
                }
            }
            index += 1
        }
        guard starts.first?.0 == 0, starts.count <= 1024 else { throw VideoDecodeError.invalidFrame }
        var units: [Data] = []
        for (number, start) in starts.enumerated() {
            let begin = start.0 + start.1
            let end = number + 1 < starts.count ? starts[number + 1].0 : bytes.count
            guard end > begin, bytes[begin] & 0x80 == 0 else { throw VideoDecodeError.invalidFrame }
            units.append(Data(bytes[begin..<end]))
        }
        return units
    }
    func decode(_ data: Data, streamID: UInt64, width: Int, height: Int, encodedWidth: Int, encodedHeight: Int) throws -> CGImage? {
        guard width > 0, height > 0, width <= 4096, height <= 4096, width * height <= 8_000_000,
              encodedWidth == width + width % 2, encodedHeight == height + height % 2,
              data.count <= 12 * 1024 * 1024 else { throw VideoDecodeError.invalidFrame }
        if stream != streamID { reset(); stream = streamID }
        let units = try Self.nalUnits(data)
        let nextSPS = units.first { $0.first.map { $0 & 31 == 7 } ?? false } ?? sps
        let nextPPS = units.first { $0.first.map { $0 & 31 == 8 } ?? false } ?? pps
        guard !nextSPS.isEmpty, !nextPPS.isEmpty else { throw VideoDecodeError.invalidFrame }
        if session == nil || nextSPS != sps || nextPPS != pps {
            reset(); sps = nextSPS; pps = nextPPS
            try createSession(encodedWidth: encodedWidth, encodedHeight: encodedHeight)
        }
        guard let format = format, let session = session else { throw VideoDecodeError.invalidFrame }
        let dimensions = CMVideoFormatDescriptionGetDimensions(format)
        guard dimensions.width == encodedWidth, dimensions.height == encodedHeight else { throw VideoDecodeError.invalidFrame }
        var accessUnit = Data()
        var hasPicture = false
        for unit in units {
            let type = unit[unit.startIndex] & 31
            if type == 7 || type == 8 || type == 9 { continue }
            if type == 1 || type == 5 { hasPicture = true }
            var length = UInt32(unit.count).bigEndian
            withUnsafeBytes(of: &length) { accessUnit.append(contentsOf: $0) }
            accessUnit.append(unit)
        }
        guard hasPicture else { throw VideoDecodeError.invalidFrame }
        var block: CMBlockBuffer?
        var status = CMBlockBufferCreateWithMemoryBlock(allocator: kCFAllocatorDefault, memoryBlock: nil,
            blockLength: accessUnit.count, blockAllocator: kCFAllocatorDefault, customBlockSource: nil,
            offsetToData: 0, dataLength: accessUnit.count, flags: 0, blockBufferOut: &block)
        guard status == noErr, let block = block else { throw VideoDecodeError.decoder(status) }
        status = accessUnit.withUnsafeBytes { bytes in
            CMBlockBufferReplaceDataBytes(with: bytes.baseAddress!, blockBuffer: block, offsetIntoDestination: 0, dataLength: bytes.count)
        }
        guard status == noErr else { throw VideoDecodeError.decoder(status) }
        var sample: CMSampleBuffer?
        var size = accessUnit.count
        status = CMSampleBufferCreateReady(allocator: kCFAllocatorDefault, dataBuffer: block, formatDescription: format,
            sampleCount: 1, sampleTimingEntryCount: 0, sampleTimingArray: nil, sampleSizeEntryCount: 1,
            sampleSizeArray: &size, sampleBufferOut: &sample)
        guard status == noErr, let sample = sample else { throw VideoDecodeError.decoder(status) }
        resultLock.lock(); output = nil; resultLock.unlock()
        status = VTDecompressionSessionDecodeFrame(session, sampleBuffer: sample, flags: [], frameRefcon: nil, infoFlagsOut: nil)
        guard status == noErr else { throw VideoDecodeError.decoder(status) }
        VTDecompressionSessionWaitForAsynchronousFrames(session)
        resultLock.lock(); let pixelBuffer = output; output = nil; resultLock.unlock()
        guard let pixelBuffer = pixelBuffer,
              CVPixelBufferGetWidth(pixelBuffer) == encodedWidth, CVPixelBufferGetHeight(pixelBuffer) == encodedHeight,
              let image = context.createCGImage(CIImage(cvPixelBuffer: pixelBuffer), from: CGRect(x: 0, y: 0, width: encodedWidth, height: encodedHeight))
        else { throw VideoDecodeError.invalidFrame }
        return image.cropping(to: CGRect(x: 0, y: 0, width: width, height: height))
    }
    private func createSession(encodedWidth: Int, encodedHeight: Int) throws {
        let status = sps.withUnsafeBytes { spsBytes in
            pps.withUnsafeBytes { ppsBytes in
                let pointers = [spsBytes.bindMemory(to: UInt8.self).baseAddress!, ppsBytes.bindMemory(to: UInt8.self).baseAddress!]
                let sizes = [sps.count, pps.count]
                return pointers.withUnsafeBufferPointer { pointerBuffer in
                    sizes.withUnsafeBufferPointer { sizeBuffer in
                        CMVideoFormatDescriptionCreateFromH264ParameterSets(allocator: kCFAllocatorDefault,
                            parameterSetCount: 2, parameterSetPointers: pointerBuffer.baseAddress!,
                            parameterSetSizes: sizeBuffer.baseAddress!, nalUnitHeaderLength: 4, formatDescriptionOut: &format)
                    }
                }
            }
        }
        guard status == noErr, let format = format else { throw VideoDecodeError.decoder(status) }
        let dimensions = CMVideoFormatDescriptionGetDimensions(format)
        guard dimensions.width == encodedWidth, dimensions.height == encodedHeight else { throw VideoDecodeError.invalidFrame }
        var callback = VTDecompressionOutputCallbackRecord(decompressionOutputCallback: { refcon, _, status, _, image, _, _ in
            guard status == noErr, let refcon = refcon else { return }
            let decoder = Unmanaged<H264Decoder>.fromOpaque(refcon).takeUnretainedValue()
            decoder.resultLock.lock(); decoder.output = image; decoder.resultLock.unlock()
        }, decompressionOutputRefCon: Unmanaged.passUnretained(self).toOpaque())
        let attributes: [CFString: Any] = [kCVPixelBufferPixelFormatTypeKey: kCVPixelFormatType_32BGRA,
            kCVPixelBufferWidthKey: encodedWidth, kCVPixelBufferHeightKey: encodedHeight,
            kCVPixelBufferIOSurfacePropertiesKey: [:]]
        let specification: [CFString: Any] = [kVTVideoDecoderSpecification_EnableHardwareAcceleratedVideoDecoder: true]
        let result = VTDecompressionSessionCreate(allocator: kCFAllocatorDefault, formatDescription: format,
            decoderSpecification: specification as CFDictionary, imageBufferAttributes: attributes as CFDictionary,
            outputCallback: &callback, decompressionSessionOut: &session)
        guard result == noErr else { throw VideoDecodeError.decoder(result) }
        var accelerated: Unmanaged<CFTypeRef>?
        if let session = session,
           VTSessionCopyProperty(session, key: kVTDecompressionPropertyKey_UsingHardwareAcceleratedVideoDecoder,
                                 allocator: kCFAllocatorDefault, valueOut: &accelerated) == noErr {
            hardwareAccelerated = accelerated?.takeRetainedValue() as? Bool ?? false
        }
    }
}
