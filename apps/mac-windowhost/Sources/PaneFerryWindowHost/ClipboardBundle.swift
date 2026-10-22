import AppKit

let clipboardByteLimit: UInt64 = 1024 * 1024 * 1024
let clipboardImageLimit = 64 * 1024 * 1024
struct ClipboardEntry: Codable {
    let path: String
    let size: UInt64
    let directory: Bool
}
struct ClipboardManifest: Codable {
    let kind: String
    let entries: [ClipboardEntry]
    func validate() throws {
        guard ["image", "files"].contains(kind), !entries.isEmpty, entries.count <= 4096 else { throw ClipboardFailure.invalid }
        var names = Set<String>(), foldedNames = Set<String>(), total: UInt64 = 0
        for entry in entries {
            let parts = entry.path.split(separator: "/", omittingEmptySubsequences: false)
            guard !entry.path.isEmpty, entry.path.utf8.count <= 4096, !entry.path.unicodeScalars.contains(where: { [0, 92, 13, 10].contains($0.value) }),
                  !parts.contains(where: { $0.isEmpty || $0 == "." || $0 == ".." }), names.insert(entry.path).inserted,
                  foldedNames.insert(entry.path.folding(options: .caseInsensitive, locale: Locale(identifier: "en_US_POSIX"))).inserted,
                  !entry.directory || entry.size == 0, entry.size <= clipboardByteLimit - total else { throw ClipboardFailure.invalid }
            total += entry.size
        }
        let directories = Set(entries.filter { $0.directory }.map { $0.path })
        for entry in entries {
            let parts = entry.path.split(separator: "/")
            if parts.count > 1 {
                for count in 1..<parts.count {
                    let parent = parts.prefix(count).joined(separator: "/")
                    guard directories.contains(parent) else { throw ClipboardFailure.invalid }
                }
            }
        }
        if kind == "image" {
            guard entries.count == 1, !entries[0].directory, total > 0, total <= clipboardImageLimit else { throw ClipboardFailure.invalid }
        }
    }
}
enum ClipboardFailure: Error { case invalid, unsupported, transport, changed }
enum ClipboardSource { case file(URL), data(Data) }
struct ClipboardBundle {
    let manifest: ClipboardManifest
    let sources: [ClipboardSource]
}
enum ControllerClipboard {
    case text(String)
    case bundle(ClipboardBundle)
    static func read(_ board: NSPasteboard) throws -> ControllerClipboard? {
        if let urls = board.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL], !urls.isEmpty {
            var entries: [ClipboardEntry] = [], sources: [ClipboardSource] = []
            func visit(_ url: URL, _ relative: String) throws {
                guard entries.count < 4096 else { throw ClipboardFailure.invalid }
                let info = try url.resourceValues(forKeys: [.isRegularFileKey, .isDirectoryKey, .isSymbolicLinkKey, .fileSizeKey])
                guard info.isSymbolicLink != true, info.isRegularFile == true || info.isDirectory == true else { throw ClipboardFailure.unsupported }
                entries.append(ClipboardEntry(path: relative, size: info.isDirectory == true ? 0 : UInt64(info.fileSize ?? 0), directory: info.isDirectory == true))
                sources.append(.file(url))
                if info.isDirectory == true {
                    for child in try FileManager.default.contentsOfDirectory(at: url, includingPropertiesForKeys: nil).sorted(by: { $0.lastPathComponent < $1.lastPathComponent }) {
                        try visit(child, relative + "/" + child.lastPathComponent)
                    }
                }
            }
            for url in urls { try visit(url, url.lastPathComponent) }
            let manifest = ClipboardManifest(kind: "files", entries: entries); try manifest.validate()
            return .bundle(ClipboardBundle(manifest: manifest, sources: sources))
        }
        var png = board.data(forType: .png)
        if png == nil, let tiff = board.data(forType: .tiff), let image = NSBitmapImageRep(data: tiff) {
            guard image.pixelsWide > 0, image.pixelsHigh > 0, image.pixelsWide <= 64_000_000 / image.pixelsHigh else { throw ClipboardFailure.invalid }
            png = image.representation(using: .png, properties: [:])
        }
        if let png = png {
            try validatePNG(png)
            let manifest = ClipboardManifest(kind: "image", entries: [ClipboardEntry(path: "image.png", size: UInt64(png.count), directory: false)])
            return .bundle(ClipboardBundle(manifest: manifest, sources: [.data(png)]))
        }
        if let text = board.string(forType: .string), !text.isEmpty {
            guard text.utf8.count <= controllerTextLimit, !text.contains("\0") else { throw ClipboardFailure.invalid }
            return .text(text)
        }
        return nil
    }
    static func validatePNG(_ data: Data) throws {
        guard data.count >= 24, data.count <= clipboardImageLimit, data.prefix(8) == Data([137,80,78,71,13,10,26,10]), String(data: data[12..<16], encoding: .ascii) == "IHDR" else { throw ClipboardFailure.invalid }
        let width = data[16..<20].reduce(UInt64(0)) { ($0 << 8) | UInt64($1) }
        let height = data[20..<24].reduce(UInt64(0)) { ($0 << 8) | UInt64($1) }
        guard width > 0, height > 0, width * height <= 64_000_000 else { throw ClipboardFailure.invalid }
    }
    static func write(_ directory: URL, manifest: ClipboardManifest, to board: NSPasteboard) throws {
        try manifest.validate()
        if manifest.kind == "image" {
            let data = try Data(contentsOf: directory.appendingPathComponent(manifest.entries[0].path)); try validatePNG(data)
            let item = NSPasteboardItem(); item.setData(data, forType: .png)
            if let rep = NSBitmapImageRep(data: data), let tiff = rep.tiffRepresentation { item.setData(tiff, forType: .tiff) }
            board.clearContents(); guard board.writeObjects([item]) else { throw ClipboardFailure.invalid }
        } else {
            var roots: [String] = []
            for entry in manifest.entries { let root = String(entry.path.split(separator: "/")[0]); if !roots.contains(root) { roots.append(root) } }
            let urls = roots.map { directory.appendingPathComponent($0) as NSURL }
            board.clearContents(); guard board.writeObjects(urls) else { throw ClipboardFailure.invalid }
        }
    }
}
