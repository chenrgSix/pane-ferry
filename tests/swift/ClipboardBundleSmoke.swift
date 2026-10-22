import AppKit

func verifyClipboardBundles() throws {
    let board = NSPasteboard.withUniqueName()
    defer { board.releaseGlobally() }
    let root = FileManager.default.temporaryDirectory.appendingPathComponent("paneferry-bundle-" + UUID().uuidString)
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: false)
    defer { try? FileManager.default.removeItem(at: root) }
    let image = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: 128, pixelsHigh: 64, bitsPerSample: 8,
        samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    image.bitmapData!.initialize(repeating: 120, count: image.bytesPerRow * image.pixelsHigh)
    let png = image.representation(using: .png, properties: [:])!
    board.setData(png, forType: .png)
    guard case .bundle(let screenshot)? = try ControllerClipboard.read(board) else { preconditionFailure("PNG must stay an image") }
    precondition(screenshot.manifest.kind == "image" && screenshot.manifest.entries[0].size == png.count)
    board.clearContents(); board.setData(image.tiffRepresentation!, forType: .tiff)
    guard case .bundle(let converted)? = try ControllerClipboard.read(board), case .data(let convertedPNG) = converted.sources[0],
          let decoded = NSBitmapImageRep(data: convertedPNG) else { preconditionFailure("screenshot TIFF converts to PNG") }
    precondition(decoded.pixelsWide == 128 && decoded.pixelsHigh == 64, "screenshot conversion retains all pixels")
    let file = root.appendingPathComponent("report 中文.txt")
    let content = Data((0..<200_000).map { UInt8($0 % 251) }); try content.write(to: file)
    let folder = root.appendingPathComponent("folder"); try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: false)
    try Data().write(to: folder.appendingPathComponent("empty"))
    board.clearContents(); board.writeObjects([file as NSURL, folder as NSURL])
    guard case .bundle(let files)? = try ControllerClipboard.read(board) else { preconditionFailure("native file URLs must stay files") }
    precondition(files.manifest.kind == "files" && files.manifest.entries.contains { $0.path == "folder/empty" && $0.size == 0 })
    let stream = root.appendingPathComponent("stream")
    var bytes = try JSONEncoder().encode(files.manifest); bytes.append(10)
    for (entry, source) in zip(files.manifest.entries, files.sources) {
        if !entry.directory, case .file(let url) = source { bytes.append(try Data(contentsOf: url)) }
    }
    try bytes.write(to: stream)
    let received = root.appendingPathComponent("received"); try FileManager.default.createDirectory(at: received, withIntermediateDirectories: false)
    let input = try FileHandle(forReadingFrom: stream); defer { try? input.close() }
    let manifest = try ClipboardBulkTransfer.receive(from: input, into: received)
    let receivedContent = try Data(contentsOf: received.appendingPathComponent("report 中文.txt"))
    precondition(receivedContent == content)
    try ControllerClipboard.write(received, manifest: manifest, to: board)
    guard case .bundle(let roundTrip)? = try ControllerClipboard.read(board) else { preconditionFailure("received file clipboard is native") }
    precondition(roundTrip.manifest.entries.count == files.manifest.entries.count)
    for path in ["../escape", "/absolute", "a/../escape", "a\\escape", "a\nfile"] {
        do { try ClipboardManifest(kind: "files", entries: [ClipboardEntry(path: path, size: 1, directory: false)]).validate(); preconditionFailure("unsafe manifest accepted") }
        catch ClipboardFailure.invalid { }
    }
    precondition(!ClipboardBulkTransfer.validID("a;id") && !ClipboardBulkTransfer.validID("../file"))
    print("PASS: private AppKit PNG/TIFF clipboard, original image dimensions, native file/folder URLs, binary stream integrity and path validation")
}
