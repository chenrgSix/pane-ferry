import Foundation
import CoreGraphics
import Darwin

private typealias RTCEvent = @convention(c) (UnsafeMutableRawPointer?, Int32, UnsafePointer<CChar>?, UnsafePointer<UInt8>?, Int) -> Void
private final class RTCAPI {
    typealias Create = @convention(c) (Int32, UnsafePointer<CChar>?, RTCEvent?, UnsafeMutableRawPointer?) -> UnsafeMutableRawPointer?
    typealias Signal = @convention(c) (UnsafeMutableRawPointer?, UnsafePointer<CChar>?, UnsafePointer<CChar>?) -> Int32
    typealias Request = @convention(c) (UnsafeMutableRawPointer?, UInt32) -> Int32
    typealias Pair = @convention(c) (UnsafeMutableRawPointer?, UnsafeMutablePointer<CChar>?, Int) -> Int32
    typealias Close = @convention(c) (UnsafeMutableRawPointer?) -> Void
    let create: Create, description: Signal, candidate: Signal, request: Request, pair: Pair, close: Close
    static let shared = try? RTCAPI()
    private init() throws {
        let path = ProcessInfo.processInfo.environment["REMOTEAPP_WEBRTC_LIBRARY"] ??
            Bundle.main.bundleURL.appendingPathComponent("Contents/Frameworks/libremoteapp_webrtc.dylib").path
        guard let library = dlopen(path, RTLD_NOW | RTLD_LOCAL) else { throw HostError.message("当前构建缺少 WebRTC 组件") }
        func symbol<T>(_ name: String, _ type: T.Type) throws -> T {
            guard let address = dlsym(library, name) else { throw HostError.message("WebRTC 组件版本不匹配") }
            return unsafeBitCast(address, to: type)
        }
        create = try symbol("ra_rtc_create", Create.self)
        description = try symbol("ra_rtc_description", Signal.self)
        candidate = try symbol("ra_rtc_candidate", Signal.self)
        request = try symbol("ra_rtc_request_keyframe", Request.self)
        pair = try symbol("ra_rtc_selected_pair", Pair.self)
        close = try symbol("ra_rtc_close", Close.self)
        // Process-global WebRTC worker state can outlive an individual peer.
        // Keep this library loaded for the process, as with VideoToolbox.
    }
}
private final class RTCEvents { weak var owner: WebRTCPeer? }
private struct RTCFrame {
    let bytes: Data
    let window: UInt32
    let received: UInt64
}

/// One authenticated media peer per SSH connection; no browser renderer or second encoder.
final class WebRTCPeer {
    static var available: Bool { RTCAPI.shared != nil }
    private let api: RTCAPI
    private var handle: UnsafeMutableRawPointer?
    private let eventOwner = RTCEvents()
    private let worker = DispatchQueue(label: "remoteapp.webrtc.media")
    private let identity = DispatchSpecificKey<Bool>()
    private let lock = NSLock()
    private var stopped = false, draining = false, resetNeeded = false
    private var queued: [RTCFrame] = []
    private var queuedBytes = 0
    private var decoders: [UInt32: H264Decoder] = [:]
    private var pixels: [UInt32: Int] = [:]
    private var lastFrames: [UInt32: UInt64] = [:]
    private var lastStreams: [UInt32: UInt64] = [:]
    private var waitingForKeyframe = Set<UInt32>()
    private var geometry: [UInt32: [String: Any]] = [:]
    private var pending: [UInt32: Incoming] = [:]
    private var retired = Set<UInt32>()
    private(set) var candidatePair: String?
    private let signal: ([String: Any]) -> Void
    private let frame: (Incoming) -> Void
    private let failure: () -> Void

    init(signal: @escaping ([String: Any]) -> Void, frame: @escaping (Incoming) -> Void, failure: @escaping () -> Void) throws {
        guard let api = RTCAPI.shared else { throw HostError.message("当前构建缺少 WebRTC 组件") }
        self.api = api; self.signal = signal; self.frame = frame; self.failure = failure
        let events = Unmanaged.passUnretained(eventOwner).toOpaque()
        let callback: RTCEvent = { context, kind, label, bytes, size in
            guard let context = context, let label = label, size >= 0,
                  size <= 12 * 1024 * 1024 + 4100,
                  size == 0 || bytes != nil,
                  let owner = Unmanaged<RTCEvents>.fromOpaque(context).takeUnretainedValue().owner else { return }
            let name = String(cString: label)
            if kind == 3 {
                guard let id = UInt32(name), id > 0, let bytes = bytes else { return }
                let received = DispatchTime.now().uptimeNanoseconds
                owner.accept(RTCFrame(bytes: Data(bytes: bytes, count: size), window: id, received: received))
            } else {
                guard size <= 48 * 1024 else { return }
                let data = size == 0 ? Data() : Data(bytes: bytes!, count: size)
                owner.worker.async { [weak owner] in owner?.event(kind, name: name, data: data) }
            }
        }
        let bind = ProcessInfo.processInfo.environment["REMOTEAPP_WEBRTC_BIND"]
        if let bind = bind { handle = bind.withCString { api.create(0, $0, callback, events) } }
        else { handle = api.create(0, nil, callback, events) }
        guard handle != nil else {
            throw HostError.message("无法初始化 WebRTC")
        }
        worker.setSpecific(key: identity, value: true)
        Unmanaged<RTCEvents>.fromOpaque(events).takeUnretainedValue().owner = self
    }
    deinit { close() }
    private var active: Bool { lock.lock(); defer { lock.unlock() }; return !stopped }
    func close() {
        lock.lock(); let closed = stopped; stopped = true; queued.removeAll(); queuedBytes = 0; lock.unlock()
        if closed { return }
        let cleanup = {
            if let handle = self.handle { self.api.close(handle); self.handle = nil }
            self.pending.removeAll(); self.decoders.removeAll(); self.pixels.removeAll()
        }
        if DispatchQueue.getSpecific(key: identity) == true { cleanup() }
        else { worker.sync(execute: cleanup) }
    }
    func description(_ sdp: String) {
        guard sdp.utf8.count <= 48 * 1024, !sdp.contains("\0") else { failure(); return }
        worker.async { [weak self] in
            guard let self = self, self.active, let handle = self.handle else { return }
            let status = sdp.withCString { value in "offer".withCString { self.api.description(handle, value, $0) } }
            if status < 0 { self.failure() }
        }
    }
    func remoteCandidate(_ candidate: String, mid: String) {
        guard candidate.utf8.count <= 2048, candidate.hasPrefix("candidate:"), !candidate.contains("\0"),
              mid.count <= 10, UInt32(mid) != nil else { failure(); return }
        worker.async { [weak self] in
            guard let self = self, self.active, let handle = self.handle else { return }
            let status = candidate.withCString { value in mid.withCString { self.api.candidate(handle, value, $0) } }
            if status < 0 { self.failure() }
        }
    }
    private func event(_ kind: Int32, name: String, data: Data) {
        guard active else { return }
        switch kind {
        case 1 where name == "answer":
            if let sdp = String(data: data, encoding: .utf8) { signal(["type":"webrtc.answer", "sdp":sdp]) }
        case 2:
            if let candidate = String(data: data, encoding: .utf8) { signal(["type":"webrtc.candidate", "candidate":candidate, "mid":name]) }
        case 5:
            if name == "2", let handle = handle {
                var bytes = [CChar](repeating: 0, count: 4096)
                let length = api.pair(handle, &bytes, bytes.count)
                if length > 0 { candidatePair = String(cString: bytes) }
            } else if name == "4" || name == "5" { failure() }
        case 6: failure()
        default: break // Packet loss is repaired by NACK/PLI, not a connection failure.
        }
    }
    private func accept(_ value: RTCFrame) {
        lock.lock()
        guard !stopped else { lock.unlock(); return }
        // Keep all dependent encoded frames in order. If decoding falls behind,
        // discard the entire backlog and request IDRs, never skip a random P-frame.
        if queued.count >= 64 || queuedBytes + value.bytes.count > 24 * 1024 * 1024 {
            queued.removeAll(); queuedBytes = 0; resetNeeded = true
        }
        queued.append(value); queuedBytes += value.bytes.count
        if draining { lock.unlock(); return }
        draining = true; lock.unlock()
        worker.async { [weak self] in self?.drain() }
    }
    private func request(_ id: UInt32) {
        waitingForKeyframe.insert(id)
        if let handle = handle { _ = api.request(handle, id) }
    }
    private func drain() {
        lock.lock()
        if stopped || queued.isEmpty { draining = false; lock.unlock(); return }
        let reset = resetNeeded; resetNeeded = false
        let value = queued.removeFirst(); queuedBytes -= value.bytes.count; lock.unlock()
        if reset { for id in decoders.keys { request(id) }; decoders.removeAll(); pending.removeAll(); pixels.removeAll() }
        do { try receive(value) } catch { request(value.window) }
        // Give signaling/geometry a turn even when several media frames wait.
        worker.async { [weak self] in self?.drain() }
    }
    private func receive(_ value: RTCFrame) throws {
        guard value.bytes.count >= 5 else { throw VideoDecodeError.invalidFrame }
        let length = value.bytes.prefix(4).reduce(0) { $0 * 256 + Int($1) }
        guard length > 0, length <= 4096, 4 + length < value.bytes.count,
              var object = try JSONSerialization.jsonObject(with: value.bytes.subdata(in: 4..<4+length)) as? [String: Any],
              object["type"] as? String == "frame", object["transport"] as? String == "webrtc",
              object["codec"] as? String == "h264",
              unsignedInteger(object["version"]) == 1,
              unsignedInteger(object["window_id"]) == UInt64(value.window),
              let session = object["session_id"] as? String, session.count == 32,
              let revision = unsignedInteger(object["geometry_revision"]),
              let stream = unsignedInteger(object["stream_id"]), stream > 0,
              let number = unsignedInteger(object["frame_id"]), number > 0,
              let width = object["width"] as? Int, let height = object["height"] as? Int,
              width > 0, height > 0, width <= 4096, height <= 4096, width * height <= 8_000_000,
              let encodedWidth = object["encoded_width"] as? Int, let encodedHeight = object["encoded_height"] as? Int,
              encodedWidth == width + width % 2, encodedHeight == height + height % 2,
              !retired.contains(value.window) else { throw VideoDecodeError.invalidFrame }
        let id = value.window
        if let prior = lastFrames[id], number <= prior { return }
        if let prior = lastStreams[id], stream < prior { return }
        if let known = geometry[id], known["visible"] as? Bool == false { return }
        let data = value.bytes.subdata(in: 4+length..<value.bytes.count)
        let keyframe = try H264Decoder.nalUnits(data).contains { $0.first.map { $0 & 31 == 5 } ?? false }
        guard object["key_frame"] as? Bool == keyframe else { throw VideoDecodeError.invalidFrame }
        if !keyframe && (waitingForKeyframe.contains(id) || decoders[id] == nil || lastFrames[id].map { number - $0 != 1 } == true || lastStreams[id].map { $0 != stream } == true) {
            request(id); return
        }
        guard pixels.filter({ $0.key != id }).values.reduce(0, +) + width * height <= 16_000_000,
              decoders[id] != nil || decoders.count < 64 else { throw VideoDecodeError.invalidFrame }
        if keyframe { waitingForKeyframe.remove(id) }
        if decoders[id] == nil { decoders[id] = H264Decoder() }
        pixels[id] = width * height
        guard let image = try decoders[id]?.decode(data, streamID: stream, width: width, height: height, encodedWidth: encodedWidth, encodedHeight: encodedHeight) else { throw VideoDecodeError.invalidFrame }
        lastFrames[id] = number; lastStreams[id] = stream
        object["media_pair"] = candidatePair
        // BENCHMARK: WebRTC decode completion (no production logging).
        let incoming = Incoming(object: object, image: image)
        if let known = geometry[id], unsignedInteger(known["geometry_revision"]) == revision,
           known["width"] as? Int == width, known["height"] as? Int == height {
            frame(incoming)
        } else {
            // UDP can overtake a resize notification on the SSH control path.
            // Decode dependencies now; retain only the latest decoded image.
            if pending.count >= 64 && pending[id] == nil { throw VideoDecodeError.invalidFrame }
            pending[id] = incoming
        }
    }
    func noteControl(_ object: [String: Any]) {
        worker.async { [weak self] in
            guard let self = self, self.active else { return }
            let type = object["type"] as? String
            if type == "session.snapshot" || type == "session.exited" {
                self.pending.removeAll(); self.geometry.removeAll(); self.decoders.removeAll(); self.pixels.removeAll()
                self.lastFrames.removeAll(); self.lastStreams.removeAll()
                if type == "session.snapshot" {
                    for window in object["windows"] as? [[String: Any]] ?? [] {
                        if let id = unsignedInteger(window["window_id"]), id <= UInt32.max { self.geometry[UInt32(id)] = window }
                    }
                }
                return
            }
            guard let id = unsignedInteger(object["window_id"]), id <= UInt32.max else { return }
            let window = UInt32(id)
            if type == "window.destroyed" {
                self.retired.insert(window); self.pending.removeValue(forKey: window); self.geometry.removeValue(forKey: window)
                self.decoders.removeValue(forKey: window); self.pixels.removeValue(forKey: window)
            } else if type == "window.created" || type == "window.metadata" {
                self.geometry[window] = object
                if object["visible"] as? Bool == false {
                    self.pending.removeValue(forKey: window); self.decoders.removeValue(forKey: window); self.pixels.removeValue(forKey: window)
                } else if let pending = self.pending[window],
                          unsignedInteger(pending.object["geometry_revision"]) == unsignedInteger(object["geometry_revision"]) {
                    self.pending.removeValue(forKey: window); self.frame(pending)
                }
            }
        }
    }
}
