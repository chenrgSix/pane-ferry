import AppKit
@main
struct ClipboardBulkQUICSmoke {
    static func main() throws {
        let args = CommandLine.arguments
        guard args.count == 4 else { fatalError("HOST USER PORT AGENT") }
        let bulk = ClipboardBulkTransfer(native: NativeRoute(peer:args[1],state:args[2],helper:args[3]))
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("remoteapp-ssh-" + UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: false)
        defer { try? FileManager.default.removeItem(at: root) }
        let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: 1024, pixelsHigh: 512, bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
        arc4random_buf(rep.bitmapData!, rep.bytesPerRow * rep.pixelsHigh)
        let png = rep.representation(using: .png, properties: [:])!
        precondition(png.count > 1024 * 1024)
        let image = ClipboardBundle(manifest: ClipboardManifest(kind: "image", entries: [ClipboardEntry(path: "image.png", size: UInt64(png.count), directory: false)]), sources: [.data(png)])
        let imageID = try bulk.upload(image)
        let (receivedImage, manifest) = try bulk.download(imageID, into: root.appendingPathComponent("downloads"))
        let roundTripImage = try Data(contentsOf: receivedImage.appendingPathComponent(manifest.entries[0].path))
        precondition(roundTripImage == png)
        let file = root.appendingPathComponent("中文 file.bin"); try png.write(to: file)
        let folder = root.appendingPathComponent("folder"); try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: false)
        let empty = folder.appendingPathComponent("empty"); try Data().write(to: empty)
        let entries = [ClipboardEntry(path: file.lastPathComponent, size: UInt64(png.count), directory: false), ClipboardEntry(path: "folder", size: 0, directory: true), ClipboardEntry(path: "folder/empty", size: 0, directory: false)]
        let files = ClipboardBundle(manifest: ClipboardManifest(kind: "files", entries: entries), sources: [.file(file), .file(folder), .file(empty)])
        let fileID = try bulk.upload(files)
        let (receivedFiles, _) = try bulk.download(fileID, into: root.appendingPathComponent("downloads"))
        let roundTripFile = try Data(contentsOf: receivedFiles.appendingPathComponent(file.lastPathComponent))
        precondition(roundTripFile == png && FileManager.default.fileExists(atPath: receivedFiles.appendingPathComponent("folder/empty").path))
        print("PASS: native QUIC binary streams with actual Swift clipboard adapter, >1MiB PNG, Unicode file, directory and empty file round trips")
        let ids = try JSONSerialization.data(withJSONObject: ["ids": [imageID, fileID]])
        print(String(data: ids, encoding: .utf8)!)
    }
}
