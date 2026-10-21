import Foundation
import CoreGraphics

@main struct H264DecoderSmoke {
    static func main() throws {
        let records = try JSONSerialization.jsonObject(with: Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[1]))) as! [[String: Any]]
        var decoder = H264Decoder()
        var connection = 0
        var redFrames = 0
        var oddFrames = 0
        var hardwareFrames = 0
        for record in records {
            let next = record["connection"] as! Int
            if next != connection { decoder = H264Decoder(); connection = next }
            let width = record["width"] as! Int, height = record["height"] as! Int
            let image = try decoder.decode(Data(base64Encoded: record["data"] as! String)!,
                streamID: (record["stream_id"] as! NSNumber).uint64Value, width: width, height: height,
                encodedWidth: record["encoded_width"] as! Int, encodedHeight: record["encoded_height"] as! Int)
            guard let image = image else { fatalError("missing decoded image") }
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
        print("PASS: \(records.count) real H.264 AU decoded (hardware: \(hardwareFrames)), P-frame continuity, key response pixels, odd resize, reconnect and malformed NAL rejection")
    }
}
