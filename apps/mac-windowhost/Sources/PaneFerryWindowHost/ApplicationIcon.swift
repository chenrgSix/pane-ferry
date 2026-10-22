import AppKit
import ImageIO

/// Only the already-rasterized, bounded Ubuntu PNG reaches AppKit. SVG, URLs and
/// oversized images are ignored; a bad icon must never interrupt a connection.
enum ApplicationIcon {
    static let maximumEncodedBytes = 24 * 1024

    static func decode(_ encoded: Data) -> NSImage? {
        guard !encoded.isEmpty, encoded.count <= maximumEncodedBytes,
              let data = Data(base64Encoded: encoded), data.count <= 16 * 1024,
              data.starts(with: [137, 80, 78, 71, 13, 10, 26, 10]),
              let source = CGImageSourceCreateWithData(data as CFData, nil),
              CGImageSourceGetType(source) as String? == "public.png",
              CGImageSourceGetCount(source) == 1,
              let properties = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any],
              let width = properties[kCGImagePropertyPixelWidth] as? Int,
              let height = properties[kCGImagePropertyPixelHeight] as? Int,
              (1...512).contains(width), (1...512).contains(height),
              let image = CGImageSourceCreateImageAtIndex(source, 0, [kCGImageSourceShouldCacheImmediately: true] as CFDictionary)
        else { return nil }
        return NSImage(cgImage: image, size: NSSize(width: width, height: height))
    }

    static func read(from input: FileHandle) -> NSImage? {
        var encoded = Data()
        do {
            while encoded.count <= maximumEncodedBytes {
                let part = try readPipeChunk(from: input, maximumCount: min(4096, maximumEncodedBytes + 1 - encoded.count))
                if part.isEmpty { return decode(encoded) }
                encoded.append(part)
            }
        } catch { return nil }
        return nil
    }

    static func fallback() -> NSImage? {
        Bundle.main.url(forResource: "PaneFerry", withExtension: "png").flatMap { NSImage(contentsOf: $0) }
    }
}
