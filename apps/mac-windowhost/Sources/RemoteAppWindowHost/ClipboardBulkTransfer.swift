import Foundation

/// Length-bounded manifest followed by exact file bytes. Runs over a separate
/// channel of the same authenticated SSH connection, without shell file paths.
final class ClipboardBulkTransfer {
    let host: String, user: String, port: UInt16
    let agent: String
    init(host: String, user: String, port: UInt16, agent: String = ".local/bin/remoteapp-agent") {
        self.host = host; self.user = user; self.port = port; self.agent = agent
    }
    private func start(_ command: String) throws -> (Process, FileHandle, FileHandle, DispatchWorkItem) {
        guard !agent.isEmpty, agent.utf8.allSatisfy({ (48...57).contains($0) || (65...90).contains($0) || (97...122).contains($0) || "/._-".utf8.contains($0) }) else { throw ClipboardFailure.invalid }
        let process = Process(), input = Pipe(), output = Pipe()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/ssh")
        process.arguments = ["-F", "/dev/null", "-T", "-S", "~/.ssh/lan-remoteapp-%C", "-oBatchMode=yes", "-oStrictHostKeyChecking=yes", "-oClearAllForwardings=yes", "-oForwardAgent=no", "-oForwardX11=no", "-oPermitLocalCommand=no", "-oConnectTimeout=10", "-oServerAliveInterval=15", "-oServerAliveCountMax=3", "-p", String(port), "-l", user, "--", host, agent + " " + command]
        process.standardInput = input; process.standardOutput = output; process.standardError = FileHandle.nullDevice
        try process.run()
        let timeout = DispatchWorkItem { if process.isRunning { process.terminate() } }
        DispatchQueue.global().asyncAfter(deadline: .now() + 300, execute: timeout)
        return (process, input.fileHandleForWriting, output.fileHandleForReading, timeout)
    }
    func upload(_ bundle: ClipboardBundle) throws -> String {
        try bundle.manifest.validate(); guard bundle.sources.count == bundle.manifest.entries.count else { throw ClipboardFailure.invalid }
        let (process, input, output, timeout) = try start("clipboard-upload")
        defer { timeout.cancel(); try? input.close(); try? output.close(); if process.isRunning { process.terminate() } }
        var line = try JSONEncoder().encode(bundle.manifest); line.append(10); try input.write(contentsOf: line)
        for (entry, source) in zip(bundle.manifest.entries, bundle.sources) {
            if entry.directory { continue }
            switch source {
            case .data(let data):
                guard data.count == entry.size else { throw ClipboardFailure.changed }
                for offset in stride(from: 0, to: data.count, by: 64 * 1024) { try input.write(contentsOf: data[offset..<min(offset + 64 * 1024, data.count)]) }
            case .file(let url):
                let before = try url.resourceValues(forKeys: [.isRegularFileKey, .isSymbolicLinkKey, .fileSizeKey, .contentModificationDateKey])
                guard before.isRegularFile == true, before.isSymbolicLink != true, before.fileSize == Int(entry.size) else { throw ClipboardFailure.changed }
                let file = try FileHandle(forReadingFrom: url); defer { try? file.close() }
                var remaining = entry.size
                while remaining > 0 {
                    let chunk = try file.read(upToCount: Int(min(remaining, 64 * 1024))) ?? Data()
                    guard !chunk.isEmpty else { throw ClipboardFailure.changed }; try input.write(contentsOf: chunk); remaining -= UInt64(chunk.count)
                }
                let after = try url.resourceValues(forKeys: [.fileSizeKey, .contentModificationDateKey])
                guard before.fileSize == after.fileSize, before.contentModificationDate == after.contentModificationDate else { throw ClipboardFailure.changed }
            }
        }
        try input.close()
        let answer = try output.read(upToCount: 256) ?? Data(); process.waitUntilExit()
        guard process.terminationStatus == 0, let value = String(data: answer, encoding: .utf8)?.trimmingCharacters(in: .whitespacesAndNewlines), Self.validID(value) else { throw ClipboardFailure.transport }
        return value
    }
    static func validID(_ id: String) -> Bool {
        !id.isEmpty && id.utf8.count <= 100 && id.utf8.allSatisfy { (48...57).contains($0) || (65...90).contains($0) || (97...122).contains($0) || $0 == 45 || $0 == 95 }
    }
    func download(_ id: String, into root: URL, protecting: Set<String> = []) throws -> (URL, ClipboardManifest) {
        guard Self.validID(id) else { throw ClipboardFailure.invalid }
        let (process, input, output, timeout) = try start("clipboard-download " + id)
        defer { timeout.cancel(); try? input.close(); try? output.close(); if process.isRunning { process.terminate() } }
        try input.close()
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        let directory = root.appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
        do {
            let manifest = try Self.receive(from: output, into: directory) { incoming in try Self.prepareCache(root, incoming: incoming, protecting: protecting.union([directory.lastPathComponent])) }
            process.waitUntilExit(); guard process.terminationStatus == 0 else { throw ClipboardFailure.transport }
            return (directory, manifest)
        } catch { try? FileManager.default.removeItem(at: directory); throw error }
    }
    private static func prepareCache(_ root: URL, incoming: UInt64, protecting: Set<String>) throws {
        var total: UInt64 = 0
        for child in try FileManager.default.contentsOfDirectory(at: root, includingPropertiesForKeys: [.isDirectoryKey, .isSymbolicLinkKey, .contentModificationDateKey]) {
            guard UUID(uuidString: child.lastPathComponent) != nil else { continue }
            let info = try child.resourceValues(forKeys: [.isDirectoryKey, .isSymbolicLinkKey, .contentModificationDateKey])
            guard info.isDirectory == true, info.isSymbolicLink != true else { continue }
            if !protecting.contains(child.lastPathComponent), let changed = info.contentModificationDate, Date().timeIntervalSince(changed) > 7 * 86400 {
                try FileManager.default.removeItem(at: child); continue
            }
            if let walk = FileManager.default.enumerator(at: child, includingPropertiesForKeys: [.isRegularFileKey, .fileSizeKey]) {
                for case let file as URL in walk { let item = try file.resourceValues(forKeys: [.isRegularFileKey, .fileSizeKey]); if item.isRegularFile == true { total += UInt64(item.fileSize ?? 0) } }
            }
        }
        guard incoming <= 4 * clipboardByteLimit, total <= 4 * clipboardByteLimit - incoming else { throw ClipboardFailure.invalid }
    }
    static func receive(from file: FileHandle, into directory: URL, capacityCheck: ((UInt64) throws -> Void)? = nil) throws -> ClipboardManifest {
        var buffer = Data()
        while !buffer.contains(10) {
            let bytes = try file.read(upToCount: 64 * 1024) ?? Data()
            guard !bytes.isEmpty, buffer.count + bytes.count <= 1024 * 1024 + 64 * 1024 else { throw ClipboardFailure.invalid }; buffer.append(bytes)
        }
        let newline = buffer.firstIndex(of: 10)!; guard newline <= 1024 * 1024 else { throw ClipboardFailure.invalid }
        let manifest = try JSONDecoder().decode(ClipboardManifest.self, from: buffer.prefix(upTo: newline)); try manifest.validate()
        try capacityCheck?(manifest.entries.reduce(0) { $0 + $1.size })
        buffer.removeSubrange(buffer.startIndex...newline)
        for entry in manifest.entries {
            let destination = directory.appendingPathComponent(entry.path)
            if entry.directory { try FileManager.default.createDirectory(at: destination, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700]); continue }
            try FileManager.default.createDirectory(at: destination.deletingLastPathComponent(), withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
            guard !FileManager.default.fileExists(atPath: destination.path) else { throw ClipboardFailure.invalid }
            guard FileManager.default.createFile(atPath: destination.path, contents: nil, attributes: [.posixPermissions: 0o600]) else { throw ClipboardFailure.invalid }
            let out = try FileHandle(forWritingTo: destination); defer { try? out.close() }
            var remaining = entry.size
            while remaining > 0 {
                if buffer.isEmpty { buffer = try file.read(upToCount: Int(min(remaining, 64 * 1024))) ?? Data() }
                guard !buffer.isEmpty else { throw ClipboardFailure.transport }
                let count = Int(min(UInt64(buffer.count), remaining)); try out.write(contentsOf: buffer.prefix(count)); buffer.removeFirst(count); remaining -= UInt64(count)
            }
        }
        guard buffer.isEmpty, (try file.read(upToCount: 1) ?? Data()).isEmpty else { throw ClipboardFailure.invalid }
        if manifest.kind == "image" { try ControllerClipboard.validatePNG(Data(contentsOf: directory.appendingPathComponent(manifest.entries[0].path))) }
        return manifest
    }
}
