import Foundation
import Darwin

/// Local control only: video, input and clipboard remain on each session's own
/// RWP transport. The private socket survives a launcher restart, not host exit.
final class ApplicationBroker {
    static func path(group: String) -> String { "/tmp/pf-app-\(geteuid())/\(group).sock" }
    private var listener: DispatchSourceRead?
    private var clients: [UUID: BrokerClient] = [:]
    private var watchers: [String: [BrokerClient]] = [:]
    var handle: (([String: Any], BrokerClient) -> Void)?

    init(group: String) throws {
        let path = Self.path(group: group)
        var directory = stat()
        guard lstat((path as NSString).deletingLastPathComponent, &directory) == 0,
              directory.st_mode & S_IFMT == S_IFDIR, directory.st_uid == geteuid(), directory.st_mode & 0o077 == 0 else {
            throw HostError.message("本机应用接收目录不可用")
        }
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw HostError.message("无法创建应用接收入口") }
        var address = sockaddr_un(); address.sun_family = sa_family_t(AF_UNIX)
        let bytes = Array(path.utf8) + [0]
        guard bytes.count <= MemoryLayout.size(ofValue: address.sun_path) else { close(fd); throw HostError.message("应用接收路径过长") }
        withUnsafeMutableBytes(of: &address.sun_path) { $0.copyBytes(from: bytes) }
        let bound = withUnsafePointer(to: &address) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { bind(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) }
        }
        guard bound == 0 else { close(fd); throw HostError.message("应用接收入口已在运行，请重试连接") }
        guard chmod(path, 0o600) == 0, listen(fd, 16) == 0,
              fcntl(fd, F_SETFL, O_NONBLOCK) == 0, fcntl(fd, F_SETFD, FD_CLOEXEC) == 0 else {
            close(fd); unlink(path); throw HostError.message("无法启用应用接收入口")
        }
        let source = DispatchSource.makeReadSource(fileDescriptor: fd, queue: .main)
        source.setEventHandler { [weak self] in self?.acceptClients(fd) }
        // Leave stale socket cleanup to the launcher's startup lock. Unlinking
        // after close here could race a new owner binding the same pathname.
        source.setCancelHandler { close(fd) }
        source.resume(); listener = source
    }
    private func acceptClients(_ listener: Int32) {
        while true {
            let fd = accept(listener, nil, nil)
            guard fd >= 0 else { return }
            var uid: uid_t = 0, gid: gid_t = 0
            guard getpeereid(fd, &uid, &gid) == 0, uid == geteuid(), clients.count < 64 else { close(fd); continue }
            let client = BrokerClient(fd: fd)
            clients[client.id] = client
            client.start(request: { [weak self, weak client] request in
                guard let self = self, let client = client else { return }
                self.handle?(request, client)
            }, closed: { [weak self, weak client] in
                guard let self = self, let client = client else { return }
                self.clients.removeValue(forKey: client.id)
                for session in Array(self.watchers.keys) {
                    self.watchers[session]?.removeAll { $0 === client }
                    if self.watchers[session]?.isEmpty == true { self.watchers.removeValue(forKey: session) }
                }
            })
        }
    }
    func watch(_ session: String, client: BrokerClient) { watchers[session, default: []].append(client) }
    func closed(_ session: String) {
        for client in watchers.removeValue(forKey: session) ?? [] {
            client.reply(["version": 1, "session": session, "closed": true]); client.shutdown()
        }
    }
    func stop() {
        listener?.cancel(); listener = nil
        for client in clients.values { client.shutdown() }
        clients.removeAll(); watchers.removeAll()
    }
    deinit { stop() }
}

/// A worker owns close(fd); other threads only shutdown it. The descriptor can
/// never be reused underneath a pending recv/send when a session is detached.
final class BrokerClient {
    let id = UUID()
    private let fd: Int32
    private let lock = NSLock()
    private var stopped = false
    init(fd: Int32) {
        self.fd = fd
        _ = fcntl(fd, F_SETFD, FD_CLOEXEC)
        var one: Int32 = 1
        _ = setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &one, socklen_t(MemoryLayout.size(ofValue: one)))
        var timeout = timeval(tv_sec: 1, tv_usec: 0)
        _ = setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, socklen_t(MemoryLayout<timeval>.size))
        _ = setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &timeout, socklen_t(MemoryLayout<timeval>.size))
    }
    private var isStopped: Bool { lock.lock(); defer { lock.unlock() }; return stopped }
    func reply(_ object: [String: Any]) {
        guard var data = try? JSONSerialization.data(withJSONObject: object) else { shutdown(); return }
        data.append(10)
        lock.lock(); defer { lock.unlock() }
        guard !stopped else { return }
        data.withUnsafeBytes { raw in
            var offset = 0
            while offset < raw.count {
                let written = Darwin.send(fd, raw.baseAddress!.advanced(by: offset), raw.count - offset, 0)
                if written < 0 && errno == EINTR { continue }
                guard written > 0 else { stopped = true; Darwin.shutdown(fd, SHUT_RDWR); return }
                offset += written
            }
        }
    }
    func shutdown() {
        lock.lock(); defer { lock.unlock() }
        if !stopped { stopped = true; Darwin.shutdown(fd, SHUT_RDWR) }
    }
    func start(request: @escaping ([String: Any]) -> Void, closed: @escaping () -> Void) {
        DispatchQueue.global(qos: .utility).async { [self] in
            defer {
                lock.lock(); stopped = true; Darwin.close(fd); lock.unlock()
                DispatchQueue.main.async(execute: closed)
            }
            var data = Data(), buffer = [UInt8](repeating: 0, count: 1024)
            let deadline = Date().addingTimeInterval(5)
            var received = false
            while !isStopped {
                let count = recv(fd, &buffer, buffer.count, 0)
                if count < 0 && [EINTR, EAGAIN, EWOULDBLOCK].contains(errno) {
                    if !received && Date() >= deadline { return }
                    continue
                }
                guard count > 0, !received else { return }
                data.append(contentsOf: buffer.prefix(count))
                guard data.count <= 4096 else { return }
                if let end = data.firstIndex(of: 10) {
                    guard end == data.count - 1,
                          let object = try? JSONSerialization.jsonObject(with: data.prefix(end)) as? [String: Any] else { return }
                    received = true
                    DispatchQueue.main.async { request(object) }
                }
            }
        }
    }
}
