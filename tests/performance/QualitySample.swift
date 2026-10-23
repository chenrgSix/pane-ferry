import AppKit
import ImageIO

/// Offline half of quality_sample.rs. Uses the production decoder and view;
/// the rendered PNG is an offscreen reconstruction, not a panel screenshot.
@main
enum QualitySample {
    struct Invalid: Error {}
    static func require(_ condition: Bool) throws { if !condition { throw Invalid() } }
    static func save(_ image: CGImage, _ url: URL) throws {
        guard let target = CGImageDestinationCreateWithURL(url as CFURL, "public.png" as CFString, 1, nil) else { throw Invalid() }
        CGImageDestinationAddImage(target, image, nil)
        try require(CGImageDestinationFinalize(target))
    }
    static func bitmap(_ width: Int, _ height: Int) throws -> CGContext {
        try require(width > 0 && height > 0 && width <= 8192 && height <= 8192 && width * height <= 32_000_000)
        guard let context = CGContext(data: nil, width: width, height: height, bitsPerComponent: 8,
            bytesPerRow: width * 4, space: CGColorSpace(name: CGColorSpace.sRGB)!,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { throw Invalid() }
        return context
    }
    static func pixels(_ image: CGImage) throws -> [UInt8] {
        let context = try bitmap(image.width, image.height)
        context.draw(image, in: CGRect(x: 0, y: 0, width: image.width, height: image.height))
        return Array(UnsafeBufferPointer(start: context.data!.assumingMemoryBound(to: UInt8.self), count: image.width * image.height * 4))
    }
    static func errors(_ reference: [UInt8], _ decoded: CGImage) throws -> [String: Double] {
        let actual = try pixels(decoded)
        try require(actual.count == reference.count)
        var all = 0.0, edge = 0.0, edgeChannels = 0
        let width = decoded.width, height = decoded.height
        for y in 0..<height {
            for x in 0..<width {
                let i = (y * width + x) * 4
                // Source-defined edge mask prevents blank background dominating
                // the only reported metric. It is not a text readability score.
                let neighbours = [(x > 0 ? i - 4 : i), (y > 0 ? i - width * 4 : i)]
                let isEdge = neighbours.contains { n in (0..<3).contains { abs(Int(reference[i + $0]) - Int(reference[n + $0])) > 24 } }
                for c in 0..<3 {
                    let delta = abs(Double(reference[i + c]) - Double(actual[i + c]))
                    all += delta
                    if isEdge { edge += delta; edgeChannels += 1 }
                }
            }
        }
        return ["rgb_mae_0_255": all / Double(width * height * 3),
                "source_edge_rgb_mae_0_255": edge / Double(max(1, edgeChannels)),
                "source_edge_pixels": Double(edgeChannels / 3)]
    }
    static func draw(_ image: CGImage, points: CGSize, backing: Double) throws -> CGImage {
        let surface = SurfaceView(frame: CGRect(origin: .zero, size: points))
        surface.remoteWidth = image.width; surface.remoteHeight = image.height; surface.image = image
        let target = try bitmap(Int((points.width * backing).rounded()), Int((points.height * backing).rounded()))
        target.scaleBy(x: backing, y: backing)
        // A bitmap context starts unflipped; NSGraphicsContext's flag alone
        // does not install the transform AppKit supplies to a flipped view.
        target.translateBy(x: 0, y: points.height)
        target.scaleBy(x: 1, y: -1)
        NSGraphicsContext.saveGraphicsState()
        defer { NSGraphicsContext.restoreGraphicsState() }
        NSGraphicsContext.current = NSGraphicsContext(cgContext: target, flipped: true)
        surface.draw(surface.bounds)
        return target.makeImage()!
    }
    static func selfTest() throws {
        let context = try bitmap(16, 12)
        context.setFillColor(NSColor.red.cgColor)
        context.fill(CGRect(x: 0, y: 0, width: 16, height: 12))
        context.setFillColor(NSColor.blue.cgColor)
        context.fill(CGRect(x: 0, y: 0, width: 7, height: 3))
        let original = context.makeImage()!
        let reference = try pixels(original)
        for backing in [1.0, 2.0] {
            let rendered = try draw(original, points: CGSize(width: 16 / backing, height: 12 / backing), backing: backing)
            try require(pixels(rendered) == reference)
        }
        let fitted = try pixels(draw(original, points: CGSize(width: 24, height: 12), backing: 1))
        for y in 0..<12 {
            for x in 0..<24 {
                let i = (y * 24 + x) * 4
                if (4..<20).contains(x) {
                    let from = (y * 16 + x - 4) * 4
                    try require(Array(fitted[i..<i+3]) == Array(reference[from..<from+3]))
                } else {
                    try require(Array(fitted[i..<i+3]) == [0, 0, 0])
                }
            }
        }
        print("PASS: production SurfaceView offscreen orientation, 1:1 backing pixels at both densities, and letterbox placement")
    }
    static func main() {
        do { try run() }
        catch {
            fputs("quality sample failed: invalid input, decode/draw check or output failure\n", stderr)
            exit(1)
        }
    }
    static func run() throws {
        _ = NSApplication.shared
        let args = CommandLine.arguments
        if args.count == 2 && args[1] == "--self-test" { try selfTest(); return }
        try require(args.count == 5)
        guard let pw = Double(args[2]), let ph = Double(args[3]), let backing = Double(args[4]) else { throw Invalid() }
        try require(pw.isFinite && ph.isFinite && backing.isFinite && pw > 0 && ph > 0 && (1...4).contains(backing)
            && pw * backing <= 8192 && ph * backing <= 8192)
        let points = CGSize(width: pw, height: ph)
        let folder = URL(fileURLWithPath: args[1], isDirectory: true)
        let output = folder.appendingPathComponent("mac-analysis", isDirectory: true)
        try require(!FileManager.default.fileExists(atPath: output.path))
        let data = try Data(contentsOf: folder.appendingPathComponent("sample.json"))
        try require(data.count <= 64 * 1024 * 1024)
        guard let sample = try JSONSerialization.jsonObject(with: data) as? [String: Any], sample["schema"] as? Int == 1,
              let width = sample["width"] as? Int, let height = sample["height"] as? Int,
              let ew = sample["encoded_width"] as? Int, let eh = sample["encoded_height"] as? Int,
              let groups = sample["samples"] as? [[String: Any]], groups.count == 2,
              let source = CGImageSourceCreateWithURL(folder.appendingPathComponent("source.png") as CFURL, nil),
              let sourceImage = CGImageSourceCreateImageAtIndex(source, 0, nil) else { throw Invalid() }
        try require(width > 0 && height > 0 && width <= 4096 && height <= 4096 && width * height <= 8_000_000)
        try require(sourceImage.width == width && sourceImage.height == height && ew == width + width % 2 && eh == height + height % 2)
        let reference = try pixels(sourceImage)
        let rect = RenderGeometry.imageRect(in: CGRect(origin: .zero, size: points), width: width, height: height)
        try FileManager.default.createDirectory(at: output, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
        try save(draw(sourceImage, points: points, backing: backing), output.appendingPathComponent("source-drawn.png"))
        var results: [[String: Any]] = []
        for group in groups {
            guard let fps = group["max_fps"] as? Int, [60, 90].contains(fps),
                  let packets = group["packets"] as? [[String: Any]], packets.count == 3 else { throw Invalid() }
            let decoder = H264Decoder()
            for (index, packet) in packets.enumerated() {
                guard let encoded = packet["data"] as? String, let bytes = Data(base64Encoded: encoded),
                      let image = try decoder.decode(bytes, streamID: 1, width: width, height: height, encodedWidth: ew, encodedHeight: eh) else { throw Invalid() }
                var result: [String: Any] = try errors(reference, image)
                result["max_fps"] = fps; result["packet"] = index; result["encoded_bytes"] = bytes.count
                result["key_frame"] = packet["key_frame"]; result["decoded_pixels"] = [image.width, image.height]
                result["hardware_decode"] = decoder.hardwareAccelerated
                results.append(result)
                if index == 0 {
                    try save(image, output.appendingPathComponent("decoded-\(fps).png"))
                    try save(draw(image, points: points, backing: backing), output.appendingPathComponent("decoded-\(fps)-drawn.png"))
                }
            }
        }
        let report: [String: Any] = ["source_pixels": [width, height], "encoded_pixels": [ew, eh],
            "content_points": [pw, ph], "backing_density": backing,
            "image_rect_points": [rect.minX, rect.minY, rect.width, rect.height],
            "image_rect_backing_pixels": [rect.width * backing, rect.height * backing],
            "enlargement": [rect.width * backing / Double(width), rect.height * backing / Double(height)],
            "packets": results,
            "scope": "same-source OpenH264 / VideoToolbox and production SurfaceView offscreen draw; not live network or panel timing"]
        try JSONSerialization.data(withJSONObject: report, options: [.sortedKeys, .prettyPrinted])
            .write(to: output.appendingPathComponent("report.json"))
        print("saved Mac decode/draw report (offscreen, no remote input)")
    }
}
