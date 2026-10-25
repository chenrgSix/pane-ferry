import Foundation
import CoreGraphics
import ImageIO
import AVFoundation

@main struct H264DecoderSmoke {
    static func main() throws {
        let records = try JSONSerialization.jsonObject(with: Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[1]))) as! [[String: Any]]
        var decoder = H264Decoder()
        var connection = 0
        var redFrames = 0
        var oddFrames = 0
        var hardwareFrames = 0
        var refinements = 0
        for record in records {
            let next = record["connection"] as! Int
            if next != connection { decoder = H264Decoder(); connection = next }
            let width = record["width"] as! Int, height = record["height"] as! Int
            if record["refinement"] as? Bool == true {
                let source = CGImageSourceCreateWithData(Data(base64Encoded: record["png"] as! String)! as CFData, nil)!
                let image = CGImageSourceCreateImageAtIndex(source, 0, nil)!
                precondition(image.width == width && image.height == height)
                refinements += 1
                continue // Retain the decoder's references for the next P-frame.
            }
            let frame = try decoder.decodeFrame(Data(base64Encoded: record["data"] as! String)!,
                streamID: (record["stream_id"] as! NSNumber).uint64Value, width: width, height: height,
                encodedWidth: record["encoded_width"] as! Int, encodedHeight: record["encoded_height"] as! Int)
            guard let frame = frame, let image = frame.image else { fatalError("missing decoded image") }
            precondition(frame.pixelBuffer != nil && frame.sample != nil, "production decode must retain CoreVideo output")
            let aperture = CMVideoFormatDescriptionGetCleanAperture(CMSampleBufferGetFormatDescription(frame.sample!)!, originIsAtTopLeft: true)
            precondition(aperture == CGRect(x: 0, y: 0, width: width, height: height))
            if decoder.hardwareAccelerated { hardwareFrames += 1 }
            precondition(image.width == width && image.height == height)
            if width % 2 != 0 { oddFrames += 1 }
            var pixels = [UInt8](repeating: 0, count: width * height * 4)
            let color = CGColorSpaceCreateDeviceRGB()
            pixels.withUnsafeMutableBytes { buffer in
                let context = CGContext(data: buffer.baseAddress!, width: width, height: height,
                    bitsPerComponent: 8, bytesPerRow: width * 4, space: color,
                    bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
                context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
            }
            let offset = (20 * width + 20) * 4
            if pixels[offset] > 180 && pixels[offset + 1] < 70 && pixels[offset + 2] < 70 { redFrames += 1 }
        }
        precondition(redFrames >= 2, "remote key response was not present in decoded pixels")
        precondition(oddFrames >= 2, "odd dimensions were not decoded/cropped")
        for data in [Data(), Data([0, 0, 1]), Data([0, 0, 1, 0x80]), Data([1, 2, 3, 4])] {
            do { _ = try H264Decoder.nalUnits(data); fatalError("malformed NAL accepted") }
            catch {}
        }
        print("PASS: \(records.count) real frames decoded (hardware: \(hardwareFrames), lossless refinements: \(refinements)), P-frame continuity, key response pixels, odd resize, reconnect and malformed NAL rejection")
    }
}
