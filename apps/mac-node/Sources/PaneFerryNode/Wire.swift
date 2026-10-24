import Foundation
import Darwin

let wireLimit = 16 * 1024 * 1024
let pixelLimit = 8_000_000
enum NodeError: String, Error {
    case invalid_request, sharing_disabled, screen_recording_required, accessibility_required
    case app_not_approved, session_exited, session_controlled, session_expired
    case request_id_conflict, session_limit, window_unavailable, input_unsupported
    case clipboard_copy_failed, clipboard_invalid, desktop_locked, capture_failed
}
func identifier(_ value: String) -> Bool {
    !value.isEmpty && value.utf8.count <= 100 && value.utf8.allSatisfy {
        (48...57).contains($0) || (65...90).contains($0) || (97...122).contains($0) || $0 == 45 || $0 == 95
    }
}
func dimensions(_ width: Int, _ height: Int) -> Bool {
    width > 0 && height > 0 && width <= 4096 && height <= 4096 && width * height <= pixelLimit
}
func privateDirectory(_ path: URL) throws {
    try FileManager.default.createDirectory(at: path, withIntermediateDirectories: true,
                                          attributes: [.posixPermissions: 0o700])
    let values = try path.resourceValues(forKeys: [.isDirectoryKey, .isSymbolicLinkKey])
    guard values.isDirectory == true, values.isSymbolicLink != true else { throw NodeError.invalid_request }
    guard chmod(path.path, 0o700) == 0 else { throw NodeError.invalid_request }
}
func savePrivate(_ value: Any, to path: URL) throws {
    let data = try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
    let temporary = path.deletingLastPathComponent().appendingPathComponent(".save-\(UUID().uuidString)")
    guard FileManager.default.createFile(atPath: temporary.path, contents: data, attributes: [.posixPermissions: 0o600]) else { throw NodeError.invalid_request }
    defer { try? FileManager.default.removeItem(at: temporary) }
    guard rename(temporary.path, path.path) == 0 else { throw NodeError.invalid_request }
}

final class Wire {
    let fd: Int32
    private let lock = NSLock()
    private var ended = false, sequence: UInt64 = 0
    private var buffer = Data()
    private let writer = DispatchQueue(label: "paneferry.publisher.writer")
    private var pending = [Data](), queuedBytes = 0, writing = false
    init(_ fd: Int32) {
        self.fd = fd
        var yes: Int32 = 1; setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &yes, socklen_t(MemoryLayout.size(ofValue: yes)))
        var timeout = timeval(tv_sec: 3, tv_usec: 0)
        setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &timeout, socklen_t(MemoryLayout.size(ofValue: timeout)))
    }
    deinit { finish(); Darwin.close(fd) }
    func finish() { lock.lock(); defer { lock.unlock() }; if !ended { ended = true; shutdown(fd, SHUT_RDWR) } }
    @discardableResult func send(_ message: [String: Any], session: String = "") -> Bool {
        lock.lock(); defer { lock.unlock() }
        guard !ended else { return false }
        sequence += 1
        var body = message; body["version"] = 1; body["seq"] = sequence; body["session_id"] = session
        guard var data = try? JSONSerialization.data(withJSONObject: body), data.count < wireLimit else { return false }
        data.append(10)
        guard pending.count < 128, queuedBytes + data.count <= 32 * 1024 * 1024 else {
            ended=true; pending.removeAll(); shutdown(fd, SHUT_RDWR); return false
        }
        queuedBytes += data.count; pending.append(data)
        if !writing { writing=true; writer.async { [self] in drain() } }
        return true
    }
    private func drain() {
        while true {
            lock.lock()
            guard !ended, !pending.isEmpty else { pending.removeAll(); queuedBytes=0; writing=false; lock.unlock(); return }
            let data=pending.removeFirst(); lock.unlock()
            let deadline=ProcessInfo.processInfo.systemUptime+3
            let success=data.withUnsafeBytes { bytes -> Bool in
                var offset=0
                while offset<bytes.count {
                    if ProcessInfo.processInfo.systemUptime >= deadline { return false }
                    let count=Darwin.send(fd,bytes.baseAddress!.advanced(by:offset),bytes.count-offset,MSG_DONTWAIT)
                    if count>0 { offset+=count; continue }
                    if count<0 && errno==EINTR { continue }
                    if count<0 && (errno==EAGAIN || errno==EWOULDBLOCK) {
                        var descriptor=pollfd(fd:fd,events:Int16(POLLOUT),revents:0)
                        if poll(&descriptor,1,100)<0 && errno != EINTR { return false }
                        continue
                    }
                    return false
                }
                return true
            }
            lock.lock(); queuedBytes-=data.count; lock.unlock()
            if !success { finish() }
        }
    }
    func read() throws -> [String: Any]? {
        while true {
            if let index = buffer.firstIndex(of: 10) {
                let line = buffer.prefix(upTo: index); buffer.removeSubrange(...index)
                guard let body = try JSONSerialization.jsonObject(with: line) as? [String: Any],
                      let version=body["version"] as? NSNumber,CFGetTypeID(version) != CFBooleanGetTypeID(),version.intValue == 1,
                      let type = body["type"] as? String,
                      line.count <= 65536 || type == "clipboard.inline.paste" else { throw NodeError.invalid_request }
                return body
            }
            guard buffer.count < wireLimit else { throw NodeError.invalid_request }
            var bytes = [UInt8](repeating: 0, count: 65536)
            let count = Darwin.read(fd, &bytes, bytes.count)
            if count == 0 { if buffer.isEmpty { return nil }; throw NodeError.invalid_request }
            if count < 0 { if errno == EINTR { continue }; return nil }
            buffer.append(contentsOf: bytes.prefix(count))
        }
    }
}

func unixSocket(_ path: String, listening: Bool) throws -> Int32 {
    let fd = socket(AF_UNIX, SOCK_STREAM, 0)
    guard fd >= 0 else { throw NodeError.invalid_request }
    var address = sockaddr_un(); address.sun_family = sa_family_t(AF_UNIX)
    let bytes = Array(path.utf8CString)
    guard bytes.count <= MemoryLayout.size(ofValue: address.sun_path) else { Darwin.close(fd); throw NodeError.invalid_request }
    withUnsafeMutableBytes(of: &address.sun_path) { $0.copyBytes(from: bytes.map { UInt8(bitPattern: $0) }) }
    let result = withUnsafePointer(to: &address) { pointer in pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
        listening ? bind(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) : connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
    }}
    guard result == 0 else { Darwin.close(fd); throw NodeError.invalid_request }
    if listening { guard chmod(path, 0o600) == 0, listen(fd, 16) == 0 else { Darwin.close(fd); throw NodeError.invalid_request } }
    return fd
}
