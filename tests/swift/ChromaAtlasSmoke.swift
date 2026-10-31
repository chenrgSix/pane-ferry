import Foundation
import CoreGraphics
import CoreVideo

@main
struct ChromaAtlasSmoke {
    static func main() throws {
        let input = try Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[1]))
        let object = try JSONSerialization.jsonObject(with: input) as! [String: Any]
        let width = object["width"] as! Int, height = object["height"] as! Int
        let ew = object["encoded_width"] as! Int, eh = object["encoded_height"] as! Int
        let bytes = Data(base64Encoded: object["data"] as! String)!
        let mode = VideoColor(rawValue: object["color"] as? String ?? "444-atlas-v1")!
        let decoder = H264Decoder()
        let frame = try decoder.decodeFrame(bytes, streamID: 1, width: width, height: height,
            encodedWidth: ew, encodedHeight: eh, colorMode: mode)!
        precondition(frame.width == width && frame.height == height)
        let image = frame.image!
        var pixels = [UInt8](repeating: 0, count: width*height*4)
        pixels.withUnsafeMutableBytes { raw in
            let context = CGContext(data: raw.baseAddress, width: width, height: height, bitsPerComponent: 8,
                bytesPerRow: width*4, space: CGColorSpaceCreateDeviceRGB(),
                bitmapInfo: CGBitmapInfo.byteOrder32Little.rawValue | CGImageAlphaInfo.premultipliedFirst.rawValue)!
            context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
        }
        for y in 4..<height-4 { for x in 4..<width-4 {
            let i = (y*width+x)*4, blue = Int(pixels[i]), red = Int(pixels[i+2])
            precondition(x%2 == 0 ? red-blue > 180 : blue-red > 180,
                "single-pixel red/blue chroma lost at \(x),\(y): \(red),\(blue)")
        }}
        do {
            _ = try decoder.decodeFrame(bytes, streamID: 2, width: width, height: height,
                encodedWidth: ew, encodedHeight: eh, colorMode: .compatible)
            preconditionFailure("atlas accepted without explicit color mode")
        } catch VideoDecodeError.invalidFrame {}
        for mode in [VideoColor.full, .fullAligned] {
            for (w,h) in [(1,1),(3,5),(1602,932),(2560,1440),(4096,1900),(1602,2049),(1920,4096)] {
                let (a,b) = try mode.dimensions(width: w, height: h)
                precondition(a <= 4096 && b <= 4096 && a*b <= 16_000_000)
                precondition(a*b >= 2*(w+w%2)*(h+h%2))
            }
        }
        let aligned = try VideoColor.fullAligned.dimensions(width: 2561, height: 1391)
        precondition(aligned.0 == 2562 && aligned.1 == 2784)
        do {
            _ = try VideoColor.fullAligned.dimensions(width: 4095, height: 1953)
            preconditionFailure("padded atlas exceeds pixel budget")
        } catch VideoDecodeError.invalidFrame {}
        print("PASS: Windows H.264 atlas decoded and full-resolution red/blue chroma restored")
    }
}
