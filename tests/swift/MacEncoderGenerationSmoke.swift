import Foundation
import CoreVideo
import Darwin
@main struct MacEncoderGenerationSmoke {
    static func main() async throws {
        var sockets=[Int32](repeating:0,count:2)
        precondition(socketpair(AF_UNIX,SOCK_STREAM,0,&sockets)==0)
        let writer=Wire(sockets[0]), reader=Wire(sockets[1])
        let rate=VideoRateController(); rate.configure(cap:60,feedback:false)
        let capture=WindowCapture(wire:writer,session:"test",windowID:1,width:639,height:480,revision:1,token:UUID(),rate:rate) { fatalError("encoder failure") }
        var image:CVPixelBuffer?
        precondition(CVPixelBufferCreate(nil,640,480,kCVPixelFormatType_32BGRA,[kCVPixelBufferIOSurfacePropertiesKey:[:]] as CFDictionary,&image)==kCVReturnSuccess)
        capture.encode(image!)
        let first=try reader.read()!
        precondition(first["key_frame"] as? Bool == true)
        // 639 and 640 have identical padded codec dimensions. Revision still
        // needs a new independent stream for receiver decoder replacement.
        try await capture.update(width:640,height:480,revision:2)
        capture.encode(image!)
        let second=try reader.read()!
        precondition(second["key_frame"] as? Bool == true)
        precondition((first["stream_id"] as! NSNumber) != (second["stream_id"] as! NSNumber))
        precondition(second["geometry_revision"] as? Int == 2 && second["width"] as? Int == 640)
        capture.encode(image!)
        let third=try reader.read()!
        precondition((third["stream_id"] as! NSNumber) == (second["stream_id"] as! NSNumber))
        capture.stop(); writer.finish(); reader.finish()
        print("PASS actual VideoToolbox IDR generation across odd/even equal padded dimensions")
    }
}
