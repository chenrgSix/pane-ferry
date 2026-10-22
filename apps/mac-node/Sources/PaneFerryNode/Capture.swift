import Foundation
import ScreenCaptureKit
import VideoToolbox
import CoreMedia

/// SCK delivers only the approved window. Coalesce raw frames before encoding;
/// encoded access units remain ordered, with a fresh IDR on every attachment.
final class WindowCapture: NSObject, SCStreamOutput, SCStreamDelegate {
    private var stream: SCStream?
    private let queue = DispatchQueue(label: "paneferry.encode", qos: .userInteractive)
    private let rawLock = NSLock()
    private var newest: CVPixelBuffer?, scheduled = false, stopped = false, reportedFailure = false
    private var encoder: VTCompressionSession?
    private var encodedSize = CGSize.zero
    private var frameNumber: Int64 = 0
    private var streamID = UInt64.random(in: 1...UInt64.max)
    let wire: Wire, session: String, windowID: UInt32
    let token: UUID
    private let failed: () -> Void
    private let metadataLock = NSLock()
    private var revision: UInt64
    private var width: Int, height: Int
    init(wire: Wire, session: String, windowID: UInt32, width: Int, height: Int, revision: UInt64, token: UUID, failed: @escaping () -> Void) {
        self.wire=wire; self.session=session; self.windowID=windowID; self.width=width; self.height=height; self.revision=revision
        self.token=token; self.failed=failed
    }
    func start(_ window: SCWindow) async throws {
        let configuration = config()
        let stream = SCStream(filter: SCContentFilter(desktopIndependentWindow: window), configuration: configuration, delegate: self)
        try stream.addStreamOutput(self, type: .screen, sampleHandlerQueue: DispatchQueue.global(qos: .userInteractive))
        self.stream=stream
        try await stream.startCapture()
        if rawLock.withLock({ stopped }) { try? await stream.stopCapture() }
    }
    private func config() -> SCStreamConfiguration {
        metadataLock.lock(); let w=width, h=height; metadataLock.unlock()
        let result=SCStreamConfiguration(); result.width=w+w%2; result.height=h+h%2
        result.minimumFrameInterval=CMTime(value: 1, timescale: 30)
        result.queueDepth=3; result.pixelFormat=kCVPixelFormatType_32BGRA
        result.showsCursor=false; result.capturesAudio=false
        if #available(macOS 14.0, *) { result.ignoreShadowsSingleWindow=true }
        return result
    }
    func update(width: Int, height: Int, revision: UInt64) async throws {
        metadataLock.withLock { self.width=width; self.height=height; self.revision=revision }
        try await stream?.updateConfiguration(config())
    }
    func stop() {
        rawLock.lock(); stopped=true; newest=nil; rawLock.unlock()
        if let stream=stream { Task { try? await stream.stopCapture() } }; stream=nil
        queue.async { [self] in if let encoder=encoder { VTCompressionSessionInvalidate(encoder) }; encoder=nil }
    }
    func stream(_ stream: SCStream, didStopWithError error: Error) {
        reportFailure()
    }
    private func reportFailure() {
        rawLock.lock(); let report = !stopped && !reportedFailure; reportedFailure=true; rawLock.unlock()
        if report { failed() }
    }
    func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .screen, sampleBuffer.isValid, let image=sampleBuffer.imageBuffer,
              let attachments=CMSampleBufferGetSampleAttachmentsArray(sampleBuffer, createIfNecessary: false) as? [[SCStreamFrameInfo:Any]],
              attachments.first?[.status] as? Int == SCFrameStatus.complete.rawValue else { return }
        rawLock.lock()
        guard !stopped else { rawLock.unlock(); return }
        newest=image
        let schedule = !scheduled; scheduled=true; rawLock.unlock()
        if schedule { queue.async { [self] in drain() } }
    }
    private func drain() {
        while true {
            rawLock.lock(); let image=newest; newest=nil
            if image == nil || stopped { scheduled=false; rawLock.unlock(); return }; rawLock.unlock()
            encode(image!)
        }
    }
    private func encode(_ image: CVPixelBuffer) {
        let w=CVPixelBufferGetWidth(image), h=CVPixelBufferGetHeight(image)
        metadataLock.lock(); let width=width, height=height, revision=revision; metadataLock.unlock()
        guard w == width+width%2, h == height+height%2, dimensions(width,height) else { return }
        if encodedSize != CGSize(width:w,height:h) || encoder == nil {
            if let encoder=encoder { VTCompressionSessionInvalidate(encoder) }; encoder=nil
            streamID=UInt64.random(in:1...UInt64.max); frameNumber=0
            let result=VTCompressionSessionCreate(allocator:nil,width:Int32(w),height:Int32(h),codecType:kCMVideoCodecType_H264,
                encoderSpecification:[kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder:true] as CFDictionary,
                imageBufferAttributes:nil,compressedDataAllocator:nil,outputCallback:{ refcon,_,status,_,sample in
                    guard status == noErr, let refcon=refcon, let sample=sample else { return }
                    Unmanaged<WindowCapture>.fromOpaque(refcon).takeUnretainedValue().output(sample)
                },refcon:Unmanaged.passUnretained(self).toOpaque(),compressionSessionOut:&encoder)
            guard result == noErr, let encoder=encoder else { reportFailure(); return }
            for (key,value) in [(kVTCompressionPropertyKey_RealTime,kCFBooleanTrue as Any),
                                (kVTCompressionPropertyKey_AllowFrameReordering,kCFBooleanFalse as Any),
                                (kVTCompressionPropertyKey_ProfileLevel,kVTProfileLevel_H264_Main_AutoLevel as Any),
                                (kVTCompressionPropertyKey_AverageBitRate,NSNumber(value:min(24_000_000,max(2_000_000,w*h*3))) as Any),
                                (kVTCompressionPropertyKey_MaxKeyFrameInterval,NSNumber(value:60) as Any),
                                (kVTCompressionPropertyKey_ExpectedFrameRate,NSNumber(value:30) as Any)] {
                VTSessionSetProperty(encoder,key:key,value:value as CFTypeRef)
            }
            guard VTCompressionSessionPrepareToEncodeFrames(encoder) == noErr else { reportFailure(); return }
            encodedSize=CGSize(width:w,height:h)
        }
        // Metadata is embedded with the frame's submission, never read from a
        // later resize in the asynchronous encoder callback.
        let info=FrameInfo(width:width,height:height,revision:revision,stream:streamID)
        pendingInfo=info
        if let encoder=encoder {
            VTCompressionSessionEncodeFrame(encoder,imageBuffer:image,presentationTimeStamp:CMTime(value:frameNumber,timescale:30),
                duration:CMTime(value:1,timescale:30),frameProperties:frameNumber == 0 ? [kVTEncodeFrameOptionKey_ForceKeyFrame:true] as CFDictionary : nil,
                sourceFrameRefcon:nil,infoFlagsOut:nil)
            VTCompressionSessionCompleteFrames(encoder,untilPresentationTimeStamp:.invalid)
        }
        frameNumber += 1
    }
    private struct FrameInfo { let width:Int, height:Int; let revision:UInt64, stream:UInt64 }
    private var pendingInfo: FrameInfo?
    private func output(_ sample: CMSampleBuffer) {
        guard let info=pendingInfo, let format=sample.formatDescription, let block=sample.dataBuffer else { return }
        let attachments=CMSampleBufferGetSampleAttachmentsArray(sample,createIfNecessary:false) as? [[CFString:Any]]
        let keyframe=attachments?.first?[kCMSampleAttachmentKey_NotSync] as? Bool != true
        var result=Data()
        if keyframe {
            for index in 0..<2 {
                var pointer: UnsafePointer<UInt8>?, count=0
                guard CMVideoFormatDescriptionGetH264ParameterSetAtIndex(format,parameterSetIndex:index,parameterSetPointerOut:&pointer,
                    parameterSetSizeOut:&count,parameterSetCountOut:nil,nalUnitHeaderLengthOut:nil) == noErr,
                    let pointer=pointer, count>0 else { reportFailure(); return }
                result.append(contentsOf:[0,0,0,1]);result.append(pointer,count:count)
            }
        }
        let length=CMBlockBufferGetDataLength(block)
        guard length>0, length<12*1024*1024 else { reportFailure();return }
        var bytes=[UInt8](repeating:0,count:length)
        guard CMBlockBufferCopyDataBytes(block,atOffset:0,dataLength:length,destination:&bytes) == noErr else { reportFailure();return }
        var offset=0
        while offset+4<=length {
            let size=bytes[offset..<offset+4].reduce(0){($0<<8)|Int($1)};offset+=4
            guard size>0, size<=length-offset else { reportFailure();return }
            result.append(contentsOf:[0,0,0,1]);result.append(contentsOf:bytes[offset..<offset+size]);offset+=size
        }
        guard offset==length else { reportFailure();return }
        // Stop and destruction must precede any future frame for this member.
        rawLock.lock(); defer { rawLock.unlock() }; guard !stopped else { return }
        wire.send(["type":"frame","window_id":windowID,"geometry_revision":info.revision,"codec":"h264",
            "width":info.width,"height":info.height,"encoded_width":info.width+info.width%2,"encoded_height":info.height+info.height%2,
            "stream_id":info.stream,"key_frame":keyframe,"data":result.base64EncodedString()],session:session)
    }
}
