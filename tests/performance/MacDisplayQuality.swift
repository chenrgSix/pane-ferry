import AppKit
import ImageIO

@main
enum MacDisplayQuality {
    static let width = 1470, height = 802
    static func bitmap(_ scale: Int = 1) -> CGContext {
        CGContext(data: nil, width: width * scale, height: height * scale, bitsPerComponent: 8,
            bytesPerRow: width * scale * 4, space: CGColorSpace(name: CGColorSpace.sRGB)!,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    }
    static func save(_ image: CGImage, _ path: URL) {
        let dest = CGImageDestinationCreateWithURL(path as CFURL, "public.png" as CFString, 1, nil)!
        CGImageDestinationAddImage(dest, image, nil); precondition(CGImageDestinationFinalize(dest))
    }
    static func png(_ data: Data) -> CGImage {
        let source = CGImageSourceCreateWithData(data as CFData, nil)!
        return CGImageSourceCreateImageAtIndex(source, 0, [kCGImageSourceShouldCacheImmediately: true] as CFDictionary)!
    }
    static func pixels(_ image: CGImage) -> [UInt8] {
        let context = bitmap(); context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
        return Array(UnsafeBufferPointer(start: context.data!.assumingMemoryBound(to: UInt8.self), count: width * height * 4))
    }
    static func generate(_ folder: URL) {
        let context = bitmap()
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(cgContext: context, flipped: false)
        NSColor.white.setFill(); NSRect(x: 0, y: 0, width: width, height: height).fill()
        NSColor(calibratedWhite: 0.95, alpha: 1).setFill(); NSRect(x: 0, y: 0, width: 260, height: height).fill()
        func text(_ value: String, _ x: Int, _ y: Int, _ size: CGFloat, _ color: NSColor = .black) {
            NSAttributedString(string: value, attributes: [.font: NSFont.systemFont(ofSize: size), .foregroundColor: color])
                .draw(at: NSPoint(x: x, y: y))
        }
        text("PaneFerry 画质对比", 28, 750, 20)
        for i in 0..<17 { text("窗口 \(i + 1) · 工作记录", 28, 695 - i * 35, 14) }
        text("相同窗口大小 · 相同源图 · 仅改变编码", 300, 750, 22)
        for i in 0..<19 {
            text("第 \(i + 1) 行：文字与控件保持原来的大小。PNG preserves source pixels 0123456789", 300, 700 - i * 31, 14)
            if i % 4 == 0 { text("链接与彩色文字 / example", 1060, 700 - i * 31, 14, .systemBlue) }
        }
        NSColor.systemBlue.setFill(); NSRect(x: 310, y: 35, width: 120, height: 36).fill()
        text("确认", 352, 44, 15, .white)
        NSGraphicsContext.restoreGraphicsState()
        let base = context.makeImage()!
        for index in 0..<60 {
            context.draw(base, in: CGRect(x: 0, y: 0, width: width, height: height))
            context.setFillColor(NSColor.systemBlue.cgColor)
            context.fill(CGRect(x: 1100 + index * 3, y: 45, width: 12, height: 12))
            save(context.makeImage()!, folder.appendingPathComponent(String(format: "frame-%03d.png", index)))
        }
    }
    static func measure(_ folder: URL) throws {
        let object = try JSONSerialization.jsonObject(with: Data(contentsOf: folder.appendingPathComponent("packets.json"))) as! [String: [String]]
        let reference = pixels(png(Data(base64Encoded: object["png"]![0])!))
        var result: [String: Any] = [:]
        for kind in ["png", "h264"] {
            let packets = object[kind]!.map { Data(base64Encoded: $0)! }
            var runs: [[String: Double]] = [], hardware = false
            for run in 0..<4 { // Warm up once, then record three identical runs.
                let decoder = H264Decoder()
                let surface = SurfaceView(frame: NSRect(x: 0, y: 0, width: width, height: height))
                surface.remoteWidth = width; surface.remoteHeight = height
                let target = bitmap(2); target.scaleBy(x: 2, y: 2)
                var decode: [Double] = [], draw: [Double] = []
                for (index, packet) in packets.enumerated() {
                    try autoreleasepool {
                        let start = ProcessInfo.processInfo.systemUptime
                        let image = kind == "png" ? png(packet) : try decoder.decode(packet, streamID: 1, width: width, height: height, encodedWidth: width, encodedHeight: height)!
                        let decoded = ProcessInfo.processInfo.systemUptime
                        NSGraphicsContext.saveGraphicsState()
                        NSGraphicsContext.current = NSGraphicsContext(cgContext: target, flipped: true)
                        surface.image = image; surface.draw(surface.bounds); target.flush()
                        NSGraphicsContext.restoreGraphicsState()
                        let drawn = ProcessInfo.processInfo.systemUptime
                        decode.append((decoded-start)*1000); draw.append((drawn-decoded)*1000)
                        if run == 0 && index == 0 {
                            save(image, folder.appendingPathComponent("\(kind)-decoded.png"))
                            let actual = pixels(image)
                            let sum = zip(reference, actual).enumerated().reduce(0.0) { value, pair in
                                pair.offset % 4 == 3 ? value : value + abs(Double(pair.element.0) - Double(pair.element.1))
                            }
                            result["\(kind)_RGB_mean_absolute_error_0_255"] = sum / Double(width * height * 3)
                        }
                    }
                }
                hardware = decoder.hardwareAccelerated
                if run > 0 {
                    let totals = zip(decode, draw).map(+).sorted()
                    let decodeMean = decode.reduce(0,+)/Double(decode.count), drawMean = draw.reduce(0,+)/Double(draw.count)
                    runs.append(["decode_ms_per_frame": decodeMean, "offscreen_draw_ms_per_frame": drawMean,
                        "processing_fps_ceiling": 1000/(decodeMean+drawMean), "p95_processing_ms": totals[Int(Double(totals.count-1)*0.95)]])
                }
            }
            result[kind] = ["runs": runs, "hardware_decode": hardware]
        }
        let data = try JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys])
        try data.write(to: folder.appendingPathComponent("measurements.json"))
    }
    static func main() throws {
        let folder = URL(fileURLWithPath: CommandLine.arguments[2])
        if CommandLine.arguments[1] == "generate" { generate(folder) } else { try measure(folder) }
    }
}
