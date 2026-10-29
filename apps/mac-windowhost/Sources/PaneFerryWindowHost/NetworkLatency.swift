import Foundation
import Darwin

enum NetworkLatencyState: Equatable {
    case measuring, unavailable
    case milliseconds(Double)
    var text: String {
        switch self {
        case .measuring: return "网络往返 测量中"
        case .unavailable: return "网络往返 暂无"
        case .milliseconds(let value):
            return value < 1 ? "网络往返 <1 ms" : String(format: "网络往返 %.0f ms", value)
        }
    }
}

/// Shared by sessions on the same device. Only local status is read; QUIC's
/// smoothed RTT is a network metric, not input-to-display or frame age.
final class NetworkLatency {
    private final class Weak { weak var value: NetworkLatency?; init(_ value: NetworkLatency) { self.value = value } }
    private static var shared: [String: Weak] = [:]
    static func monitor(_ route: NativeRoute) -> NetworkLatency {
        let key = route.socket
        if let monitor = shared[key]?.value { return monitor }
        shared = shared.filter { $0.value.value != nil }
        let monitor = NetworkLatency(route: route)
        shared[key] = Weak(monitor); return monitor
    }
    let route: NativeRoute
    private var latest: NetworkLatencyState = .measuring
    private var sampledAt: TimeInterval?
    private var requestedAt: TimeInterval?
    private var reading = false
    var read: (NativeRoute) throws -> Double? = NetworkLatency.readStatus
    var clock: () -> TimeInterval = { ProcessInfo.processInfo.systemUptime }
    init(route: NativeRoute) { self.route = route }
    var state: NetworkLatencyState {
        guard let sampledAt = sampledAt else { return latest }
        return clock() - sampledAt <= 6 ? latest : .unavailable
    }
    func refresh() {
        let now = clock()
        guard !reading, requestedAt.map({ now - $0 >= 2 }) ?? true else { return }
        reading = true; requestedAt = now
        let route = route, read = read, clock = clock
        DispatchQueue.global(qos: .utility).async { [weak self] in
            let value = try? read(route)
            let sampled = clock()
            DispatchQueue.main.async { [weak self] in
                guard let self = self else { return }
                self.reading = false; self.sampledAt = sampled
                if let value = value, value.isFinite, value >= 0 { self.latest = .milliseconds(value) }
                else { self.latest = .unavailable }
            }
        }
    }
    static func parse(_ data: Data, peer: String) throws -> Double? {
        let object = try JSONSerialization.jsonObject(with: data) as? [String: Any]
        guard let entry = (object?["peers"] as? [[String: Any]])?.first(where: { $0["device_id"] as? String == peer }),
              entry["connected"] as? Bool == true, let number = entry["rtt_ms"] as? NSNumber,
              CFGetTypeID(number) != CFBooleanGetTypeID(), number.doubleValue.isFinite, number.doubleValue >= 0 else { return nil }
        return number.doubleValue
    }
    static func readStatus(_ route: NativeRoute) throws -> Double? {
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw HostError.message("无法读取连接状态") }
        defer { close(fd) }
        guard fcntl(fd, F_SETFL, O_NONBLOCK) == 0, fcntl(fd, F_SETFD, FD_CLOEXEC) == 0 else { throw HostError.message("无法读取连接状态") }
        var one: Int32 = 1
        _ = setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &one, socklen_t(MemoryLayout.size(ofValue: one)))
        let deadline = ProcessInfo.processInfo.systemUptime + 1
        func wait(_ events: Int16) throws {
            while true {
                let remaining = deadline - ProcessInfo.processInfo.systemUptime
                guard remaining > 0 else { throw HostError.message("连接状态读取超时") }
                var descriptor = pollfd(fd: fd, events: events, revents: 0)
                let result = Darwin.poll(&descriptor, 1, Int32(max(1, remaining * 1000)))
                if result < 0 && errno == EINTR { continue }
                guard result > 0, descriptor.revents & (events | Int16(POLLHUP)) != 0 else { throw HostError.message("连接状态不可用") }
                return
            }
        }
        var address = sockaddr_un(); address.sun_family = sa_family_t(AF_UNIX)
        let path = URL(fileURLWithPath: route.state).appendingPathComponent("control.sock").path
        let bytes = Array(path.utf8) + [0]
        guard bytes.count <= MemoryLayout.size(ofValue: address.sun_path) else { throw HostError.message("连接状态路径无效") }
        withUnsafeMutableBytes(of: &address.sun_path) { $0.copyBytes(from: bytes) }
        let result = withUnsafePointer(to: &address) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) { Darwin.connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) }
        }
        if result != 0 {
            guard errno == EINPROGRESS || errno == EAGAIN else { throw HostError.message("连接状态不可用") }
            try wait(Int16(POLLOUT))
            var error: Int32 = 0, size = socklen_t(MemoryLayout<Int32>.size)
            guard getsockopt(fd, SOL_SOCKET, SO_ERROR, &error, &size) == 0, error == 0 else { throw HostError.message("连接状态不可用") }
        }
        var uid: uid_t = 0, gid: gid_t = 0
        guard getpeereid(fd, &uid, &gid) == 0, uid == geteuid() else { throw HostError.message("连接状态来源无效") }
        let request = Data("{\"op\":\"status\"}\n".utf8)
        try request.withUnsafeBytes { raw in
            var offset = 0
            while offset < raw.count {
                try wait(Int16(POLLOUT))
                let count = Darwin.send(fd, raw.baseAddress!.advanced(by: offset), raw.count - offset, 0)
                if count < 0 && [EINTR, EAGAIN, EWOULDBLOCK].contains(errno) { continue }
                guard count > 0 else { throw HostError.message("连接状态不可用") }; offset += count
            }
        }
        var data = Data(), buffer = [UInt8](repeating: 0, count: 4096)
        while data.count < 128 * 1024 {
            try wait(Int16(POLLIN))
            let count = recv(fd, &buffer, buffer.count, 0)
            if count < 0 && [EINTR, EAGAIN, EWOULDBLOCK].contains(errno) { continue }
            guard count > 0 else { throw HostError.message("连接状态不可用") }
            data.append(contentsOf: buffer.prefix(count))
            guard data.count <= 128 * 1024 else { throw HostError.message("连接状态响应过大") }
            if let end = data.firstIndex(of: 10) { return try parse(data.prefix(end), peer: route.peer) }
        }
        throw HostError.message("连接状态响应过大")
    }
}
