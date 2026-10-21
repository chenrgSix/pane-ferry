import AppKit
import Foundation
import ImageIO

// RWP/1 developer preview. Rendering is CoreGraphics/NSView, not Metal.
let maximumLine = 16 * 1024 * 1024
let maximumPixels = 8_000_000
func unsignedInteger(_ value: Any?) -> UInt64? {
    guard let number = value as? NSNumber, CFGetTypeID(number) != CFBooleanGetTypeID() else { return nil }
    let string = number.stringValue
    guard !string.isEmpty, string.utf8.allSatisfy({ $0 >= 48 && $0 <= 57 }) else { return nil }
    return UInt64(string)
}

struct Options {
    let host: String
    let user: String
    let port: UInt16
    let app: String?
    let session: String?
    let videoTransport: String
    static func parse(_ args: [String]) throws -> Options {
        var values: [String: String] = [:]
        var i = 0
        let allowed: Set<String> = ["--host", "--user", "--port", "--app", "--session", "--video-transport"]
        while i < args.count {
            guard allowed.contains(args[i]), i + 1 < args.count, values[args[i]] == nil else {
                throw HostError.message("参数格式错误；使用 --host HOST --user USER [--port 22] --app APP_ID 或 --session SESSION_ID")
            }
            values[args[i]] = args[i + 1]; i += 2
        }
        guard let host = values["--host"], host.count <= 253,
              host.range(of: "^[A-Za-z0-9][A-Za-z0-9.-]*$", options: .regularExpression) != nil,
              host.split(separator: ".", omittingEmptySubsequences: false).allSatisfy({ !$0.isEmpty && $0.count <= 63 && !$0.hasPrefix("-") && !$0.hasSuffix("-") }),
              let user = values["--user"], user.count <= 64,
              user.range(of: "^[A-Za-z0-9_][A-Za-z0-9_-]*$", options: .regularExpression) != nil,
              let port = UInt16(values["--port"] ?? "22"), port > 0 else {
            throw HostError.message("主机、SSH 用户名或端口无效。使用真实主机名/IP，不接受 SSH 别名配置或额外选项。")
        }
        let app = values["--app"], session = values["--session"]
        guard (app == nil) != (session == nil) else { throw HostError.message("必须且只能指定 --app 或 --session") }
        for id in [app, session].compactMap({ $0 }) {
            guard id.count <= 128, id.range(of: "^[A-Za-z0-9][A-Za-z0-9_.-]*$", options: .regularExpression) != nil else {
                throw HostError.message("应用/会话 ID 无效")
            }
        }
        let videoTransport = values["--video-transport"] ?? "ssh"
        guard ["ssh", "webrtc", "auto"].contains(videoTransport) else { throw HostError.message("视频传输必须是 ssh、webrtc 或 auto") }
        return Options(host: host, user: user, port: port, app: app, session: session, videoTransport: videoTransport)
    }
}
enum HostError: Error { case message(String) }

struct Incoming {
    let object: [String: Any]
    let image: CGImage?
}

/// Local SSH is the only transport. No listening port, no new keys, no shell interpolation.
final class SSHTransport {
    private let process = Process()
    private let input = Pipe()
    private let output = Pipe()
    private let errors = Pipe()
    private let lock = NSLock()
    private var inbox: [Incoming] = []
    private var writes: [Data] = []
    private var writing = false
    private var closed = false
    private var failure: String?
    private var mediaPeer: WebRTCPeer?
    private var mediaRequested = false, mediaStrict = false, mediaFrameSeen = false, mediaDeadlineStarted = false
    private var queuedPixels = 0
    private let writer = DispatchQueue(label: "remoteapp.ssh.writer")
    private(set) var alive = false
    var onDiagnostic: ((String) -> Void)?

    func start(_ options: Options) throws {
        process.executableURL = URL(fileURLWithPath: "/usr/bin/ssh")
        // Ignore SSH config: otherwise a user's ProxyCommand/RemoteCommand could change transport semantics.
        process.arguments = ["-F", "/dev/null", "-T", "-S", "~/.ssh/lan-remoteapp-%C", "-oBatchMode=yes", "-oStrictHostKeyChecking=yes",
            "-oClearAllForwardings=yes", "-oForwardAgent=no", "-oForwardX11=no", "-oPermitLocalCommand=no",
            "-oConnectTimeout=10", "-oServerAliveInterval=15", "-oServerAliveCountMax=3",
            "-p", String(options.port), "-l", options.user, "--", options.host,
            ".local/bin/remoteapp-agent bridge"]
        process.standardInput = input; process.standardOutput = output; process.standardError = errors
        // Do not expose remote stderr (which may contain private app data) in normal logs.
        process.terminationHandler = { [weak self] process in
            self?.fail("SSH 连接已结束（退出码 \(process.terminationStatus)）。远端会话不会因此主动终止；可用会话 ID 重新连接。")
        }
        try process.run(); alive = true
        DispatchQueue(label: "remoteapp.ssh.stderr").async { [weak self] in
            guard let self = self else { return }
            while true { do { if try readPipeChunk(from: self.errors.fileHandleForReading, maximumCount: 4096).isEmpty { break } } catch { break } }
        }
        DispatchQueue(label: "remoteapp.ssh.reader").async { [weak self] in self?.readLoop() }
    }

    private func fail(_ message: String) {
        lock.lock(); if failure == nil { failure = message }; lock.unlock()
    }
    func startWebRTC(strict: Bool) throws {
        let peer = try WebRTCPeer(signal: { [weak self] object in
            guard let self = self else { return }
            self.lock.lock(); let active = self.mediaRequested && !self.closed; self.lock.unlock()
            if active { self.send(object) }
        }, frame: { [weak self] incoming in
            guard let self = self else { return }
            self.lock.lock(); let active = self.mediaRequested && !self.closed; self.mediaFrameSeen = active; self.lock.unlock()
            guard active else { return }
            do { try self.enqueue(incoming) } catch { self.mediaFailed() }
        }, failure: { [weak self] in self?.mediaFailed() })
        lock.lock(); mediaPeer = peer; mediaRequested = true; mediaStrict = strict; lock.unlock()
    }
    private func mediaFailed() {
        lock.lock()
        guard mediaRequested && !closed else { lock.unlock(); return }
        mediaRequested = false
        let peer = mediaPeer; mediaPeer = nil; let strict = mediaStrict; lock.unlock()
        peer?.close()
        if strict { fail("WebRTC 媒体连接失败；本次强制测试不会悄悄回退 SSH。") }
        else { send(["type":"video.fallback"]) }
    }
    private func currentMedia() -> WebRTCPeer? { lock.lock(); defer { lock.unlock() }; return mediaPeer }
    private func mediaOffer(_ sdp: String) {
        currentMedia()?.description(sdp)
        lock.lock(); let start = mediaRequested && !mediaDeadlineStarted; mediaDeadlineStarted = true; lock.unlock()
        if start {
            DispatchQueue.global().asyncAfter(deadline: .now() + 5) { [weak self] in
                guard let self = self else { return }
                self.lock.lock(); let expired = self.mediaRequested && !self.mediaFrameSeen; self.lock.unlock()
                if expired { self.mediaFailed() }
            }
        }
    }
    func drain() -> ([Incoming], String?) {
        lock.lock(); defer { lock.unlock() }
        let events = inbox; inbox.removeAll(keepingCapacity: true); queuedPixels = 0
        let error = failure; failure = nil
        return (events, error)
    }
    private func enqueue(_ value: Incoming) throws {
        lock.lock(); defer { lock.unlock() }
        // Frame coalescing preserves control-event ordering: remove older queued frame, append newest.
        if value.object["type"] as? String == "frame", let id = value.object["window_id"] as? NSNumber {
            inbox.removeAll { $0.object["type"] as? String == "frame" && ($0.object["window_id"] as? NSNumber) == id }
        }
        queuedPixels = inbox.reduce(0) { $0 + (($1.image?.width ?? 0) * ($1.image?.height ?? 0)) }
        let pixels = (value.image?.width ?? 0) * (value.image?.height ?? 0)
        while queuedPixels + pixels > 16_000_000 {
            guard let index = inbox.firstIndex(where: { $0.image != nil }) else { break }
            let removed = inbox.remove(at: index)
            queuedPixels -= (removed.image?.width ?? 0) * (removed.image?.height ?? 0)
        }
        guard inbox.count < 256 else { throw HostError.message("事件队列超过上限，已断开以避免内存持续增长") }
        inbox.append(value); queuedPixels += pixels
    }
    private func readLoop() {
        var buffer = Data()
        var videoDecoders: [UInt32: H264Decoder] = [:]
        var videoPixels: [UInt32: Int] = [:]
        do {
            while true {
                lock.lock(); let stop = closed; lock.unlock(); if stop { break }
                let chunk = try readPipeChunk(from: output.fileHandleForReading)
                if chunk.isEmpty { break }
                buffer.append(chunk)
                while let newline = buffer.firstIndex(of: 10) {
                    let length = buffer.distance(from: buffer.startIndex, to: newline)
                    guard length <= maximumLine else { throw HostError.message("协议消息过大") }
                    let line = buffer.prefix(upTo: newline)
                    buffer.removeSubrange(buffer.startIndex...newline)
                    if line.isEmpty { continue }
                    guard let object = try JSONSerialization.jsonObject(with: Data(line)) as? [String: Any],
                          object["type"] is String,
                          unsignedInteger(object["version"]) == 1 else { throw HostError.message("RWP 协议格式或版本错误") }
                    let allowedTypes: Set<String> = ["hello", "error", "video.ready", "webrtc.offer", "webrtc.candidate", "webrtc.state", "webrtc.failed", "session.started", "session.snapshot", "session.exited", "window.created", "window.metadata", "window.destroyed", "frame", "clipboard.data"]
                    guard allowedTypes.contains(object["type"] as? String ?? "") else { throw HostError.message("未知 RWP 消息类型") }
                    if object["type"] as? String == "webrtc.offer" {
                        guard let sdp = object["sdp"] as? String else { throw HostError.message("无效 WebRTC 协商") }
                        mediaOffer(sdp); continue
                    }
                    if object["type"] as? String == "webrtc.candidate" {
                        guard let candidate = object["candidate"] as? String, let mid = object["mid"] as? String else { throw HostError.message("无效 ICE 候选") }
                        currentMedia()?.remoteCandidate(candidate, mid: mid); continue
                    }
                    if object["type"] as? String == "webrtc.failed" { mediaFailed(); continue }
                    if object["type"] as? String == "webrtc.state" { continue }
                    if ["session.snapshot", "session.exited"].contains(object["type"] as? String ?? "") { videoDecoders.removeAll(); videoPixels.removeAll() }
                    if object["type"] as? String == "window.destroyed" ||
                        (object["type"] as? String == "window.metadata" && (object["visible"] as? Bool) == false),
                        let id = unsignedInteger(object["window_id"]), id <= UInt32.max {
                        videoDecoders.removeValue(forKey: UInt32(id)); videoPixels.removeValue(forKey: UInt32(id))
                    }
                    var image: CGImage?
                    if object["type"] as? String == "frame" {
                        guard let w = object["width"] as? Int, let h = object["height"] as? Int,
                              w > 0, h > 0, w <= 4096, h <= 4096, w * h <= maximumPixels
                        else { throw HostError.message("无效或过大的视频尺寸") }
                        if object["codec"] as? String == "h264" {
                            guard let id = unsignedInteger(object["window_id"]), id <= UInt32.max,
                                  let stream = unsignedInteger(object["stream_id"]), stream > 0,
                                  let ew = object["encoded_width"] as? Int, let eh = object["encoded_height"] as? Int,
                                  let encoded = object["data"] as? String, let data = Data(base64Encoded: encoded),
                                  data.count <= 12 * 1024 * 1024 else { throw HostError.message("无效 H.264 帧") }
                            let windowID = UInt32(id)
                            guard videoPixels.filter({ $0.key != windowID }).values.reduce(0, +) + w * h <= 16_000_000 else {
                                throw HostError.message("视频解码像素总量超限")
                            }
                            videoPixels[windowID] = w * h
                            if videoDecoders[windowID] == nil {
                                guard videoDecoders.count < 64 else { throw HostError.message("视频窗口数量超限") }
                                videoDecoders[windowID] = H264Decoder()
                            }
                            image = try videoDecoders[windowID]?.decode(data, streamID: stream, width: w, height: h, encodedWidth: ew, encodedHeight: eh)
                        } else {
                            guard object["codec"] == nil,
                              let encoded = object["png"] as? String,
                              let png = Data(base64Encoded: encoded), png.count <= 12 * 1024 * 1024,
                              png.starts(with: [137,80,78,71,13,10,26,10]),
                              let source = CGImageSourceCreateWithData(png as CFData, nil),
                              CGImageSourceGetCount(source) == 1,
                              let properties = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any],
                              (properties[kCGImagePropertyPixelWidth] as? NSNumber)?.intValue == w,
                              (properties[kCGImagePropertyPixelHeight] as? NSNumber)?.intValue == h,
                              let decoded = CGImageSourceCreateImageAtIndex(source, 0, [kCGImageSourceShouldCacheImmediately: true] as CFDictionary)
                        else { throw HostError.message("无效或过大的 PNG 帧") }
                        image = decoded
                        }
                    }
                    var lightweight = object; lightweight.removeValue(forKey: "png"); lightweight.removeValue(forKey: "data")
                    try enqueue(Incoming(object: lightweight, image: image))
                    currentMedia()?.noteControl(lightweight)
                }
                guard buffer.count <= maximumLine else { throw HostError.message("协议消息未终止或过大") }
            }
            if !buffer.isEmpty { throw HostError.message("连接结束时存在不完整消息") }
        } catch { fail("RWP 数据读取失败，已停止接收。请检查版本和连接。") }
        disconnect()
    }
    func send(_ object: [String: Any]) {
        var message = object; message["version"] = 1
        guard let encoded = try? JSONSerialization.data(withJSONObject: message), encoded.count < maximumLine else { return }
        var line = encoded; line.append(10)
        lock.lock()
        guard !closed else { lock.unlock(); return }
        guard writes.count < 256 else { lock.unlock(); fail("输入队列已满，连接停止，避免输入失序"); disconnect(); return }
        writes.append(line)
        if writing { lock.unlock(); return }
        writing = true; lock.unlock()
        writer.async { [weak self] in self?.writeLoop() }
    }
    private func writeLoop() {
        while true {
            lock.lock()
            if closed || writes.isEmpty { writing = false; lock.unlock(); return }
            let next = writes.removeFirst(); lock.unlock()
            do { try input.fileHandleForWriting.write(contentsOf: next) }
            catch { fail("向远端发送消息失败"); disconnect(); return }
        }
    }
    func disconnect() {
        lock.lock(); let wasClosed = closed; closed = true; writes.removeAll()
        mediaRequested = false; let peer = mediaPeer; mediaPeer = nil; lock.unlock()
        if wasClosed { return }
        peer?.close()
        try? input.fileHandleForWriting.close()
        try? output.fileHandleForReading.close()
        if process.isRunning { process.terminate() }
    }
}


final class AppDelegate: NSObject, NSApplicationDelegate, NSMenuItemValidation {
    let options: Options
    let transport = SSHTransport()
    var windows: [UInt32: HostedWindow] = [:]
    var parents: [UInt32: UInt32] = [:]
    var removed = Set<UInt32>()
    var session: String?
    var lastSequence: UInt64 = 0
    var ready = false
    var connected = true
    var poll: Timer?
    var statusItem: NSStatusItem?
    var capabilities = Set<String>()
    var clipboardCopyGate = ClipboardCopyGate()
    init(options: Options) { self.options = options; super.init() }
    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.regular)
        let mainMenu = NSMenu()
        let appItem = NSMenuItem(); mainMenu.addItem(appItem)
        let menu = NSMenu(); appItem.submenu = menu
        menu.addItem(withTitle: "关于 LAN RemoteApp", action: #selector(showAbout), keyEquivalent: "")
        menu.addItem(withTitle: "复制会话 ID", action: #selector(copySession), keyEquivalent: "")
        menu.addItem(.separator())
        menu.addItem(withTitle: "断开并退出（保留远端应用）", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        NSApp.mainMenu = mainMenu
        let editItem = NSMenuItem(); mainMenu.addItem(editItem)
        let editMenu = NSMenu(title: "编辑"); editItem.submenu = editMenu
        editMenu.addItem(withTitle: "复制到控制端", action: #selector(SurfaceView.copy(_:)), keyEquivalent: "c")
        editMenu.addItem(withTitle: "从控制端粘贴", action: #selector(SurfaceView.paste(_:)), keyEquivalent: "v")
        let displayItem = NSMenuItem(); mainMenu.addItem(displayItem)
        let displayMenu = NSMenu(title: "显示"); displayItem.submenu = displayMenu
        displayMenu.addItem(withTitle: "自动适应窗口", action: #selector(automaticResolution), keyEquivalent: "0")
        displayMenu.addItem(withTitle: "适应可用屏幕", action: #selector(fitScreen), keyEquivalent: "")
        displayMenu.addItem(withTitle: "全屏显示", action: #selector(fullScreen), keyEquivalent: "")
        displayMenu.addItem(withTitle: "设置分辨率…", action: #selector(setResolution), keyEquivalent: "")
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        statusItem?.button?.title = "RemoteApp · 连接中"; statusItem?.menu = menu.copy() as? NSMenu
        do {
            try transport.start(options)
            transport.send(["type": "hello"])
            let timer = Timer(timeInterval: 1.0 / 60.0, repeats: true) { [weak self] _ in self?.pump() }
            RunLoop.main.add(timer, forMode: .common); poll = timer
            DispatchQueue.main.asyncAfter(deadline: .now() + 15) { [weak self] in
                guard let self = self, !self.ready, self.connected else { return }
                self.transport.disconnect()
            }
        } catch { alert("无法启动 SSH。请检查系统 SSH 与登录配置。") }
        NSApp.activate(ignoringOtherApps: true)
    }
    @objc func showAbout() {
        alert("LAN RemoteApp 开发预览\n原生 AppKit 窗口 + PNG。分辨率可随窗口调整或指定像素宽高，界面缩放独立设置为 100%/200%；原桌面窗口沿用其已配置的缩放。输入法在控制端组合，剪贴板只由主动操作触发。Metal 尚未实现。")
    }
    func activeHost() -> HostedWindow? { windows.values.first { $0.window === NSApp.keyWindow } }
    @objc func automaticResolution() { activeHost()?.useAutomaticResolution() }
    @objc func fitScreen() { activeHost()?.fitToScreen() }
    @objc func fullScreen() { activeHost()?.window.toggleFullScreen(nil) }
    func validateMenuItem(_ menuItem: NSMenuItem) -> Bool {
        if menuItem.action == #selector(automaticResolution) {
            menuItem.state = activeHost()?.fixedPixels == nil ? .on : .off
            return activeHost() != nil
        }
        return true
    }
    @objc func setResolution() {
        guard let host = activeHost() else { return }
        let dialog = NSAlert(); dialog.messageText = "设置远端窗口分辨率"
        dialog.informativeText = "输入远端实际渲染像素宽高，固定模式按比例显示。选择“自动适应窗口”可恢复随窗口调整、铺满画面的默认模式。上限 4096×2160、800 万像素。"
        let width = NSTextField(string: String(host.surface.remoteWidth)), height = NSTextField(string: String(host.surface.remoteHeight))
        let fields = NSStackView(views: [NSTextField(labelWithString: "宽"), width, NSTextField(labelWithString: "高"), height])
        fields.orientation = .horizontal; fields.spacing = 8; fields.frame = NSRect(x: 0, y: 0, width: 360, height: 28)
        dialog.accessoryView = fields; dialog.addButton(withTitle: "应用"); dialog.addButton(withTitle: "取消")
        guard dialog.runModal() == .alertFirstButtonReturn else { return }
        guard let w = Int(width.stringValue), let h = Int(height.stringValue), w >= 64, h >= 48, w <= 4096, h <= 2160, w * h <= maximumPixels else {
            alert("分辨率超出范围，请输入有效的像素宽高。"); return
        }
        host.fixedPixels = (w, h)
        host.windowDidResize(Notification(name: NSWindow.didResizeNotification))
    }
    @objc func copySession() {
        guard let session = session else { return }
        NSPasteboard.general.clearContents(); NSPasteboard.general.setString(session, forType: .string)
    }
    private func alert(_ message: String) {
        let alert = NSAlert(); alert.messageText = "LAN RemoteApp"; alert.informativeText = message
        alert.addButton(withTitle: "好"); alert.runModal()
    }
    func pump() {
        let (events, error) = transport.drain()
        for event in events { receive(event) }
        if let error = error, connected {
            connected = false; statusItem?.button?.title = "RemoteApp · 已断开"
            for host in windows.values { host.surface.transmit = nil; host.window.title += " [已断开]" }
            alert(error + (session.map { "\n会话 ID：\($0)" } ?? ""))
        }
    }
    func send(_ message: [String: Any]) {
        guard connected else { return }
        if let type = message["type"] as? String, ["input.text.commit", "clipboard.paste", "clipboard.copy"].contains(type) {
            guard capabilities.contains(type) else { NSSound.beep(); return }
            if type == "clipboard.copy", let id = message["request_id"] as? String,
               let window = message["window_id"] as? UInt32 {
                clipboardCopyGate.begin(id: id, window: window)
            }
        }
        // The bridge owns one attached session. Only session.resume carries session_id.
        transport.send(message)
    }
    func receive(_ incoming: Incoming) {
        let object = incoming.object
        guard let type = object["type"] as? String else { return }
        if type == "hello" {
            guard !ready else { return }; ready = true
            capabilities = Set(object["capabilities"] as? [String] ?? [])
            let canWebRTC = capabilities.contains("native.webrtc.h264") && WebRTCPeer.available
            if options.videoTransport == "webrtc" && !canWebRTC {
                alert("控制端或远端缺少 WebRTC 组件，本次强制测试无法开始。"); transport.disconnect(); return
            }
            if options.videoTransport != "ssh" && canWebRTC {
                do {
                    try transport.startWebRTC(strict: options.videoTransport == "webrtc")
                    send(["type":"webrtc.enable"])
                } catch { alert("WebRTC 初始化失败。"); transport.disconnect(); return }
            } else if capabilities.contains("native.h264") { send(["type":"video.enable", "codec":"h264"]) }
            if let app = options.app {
                var launch: [String: Any] = ["type": "app.launch", "app_id": app, "request_id": UUID().uuidString, "backend": "native"]
                if capabilities.contains("display.scale") { launch["scale_factor"] = (NSScreen.main?.backingScaleFactor ?? 1) >= 1.5 ? 2 : 1 }
                send(launch)
            }
            else if let session = options.session { send(["type": "session.resume", "session_id": session]) }
            return
        }
        if type == "video.ready" { statusItem?.button?.title = "RemoteApp · H.264 视频"; return }
        if type == "error" {
            // Display server error code, not an arbitrary message potentially containing app data.
            let code = object["code"] as? String ?? "unknown"
            if code == "window_unavailable" { return }
            if ["controller_text_failed", "clipboard_copy_failed"].contains(code) {
                statusItem?.button?.title = "RemoteApp · 输入或剪贴板失败"
                NSSound.beep(); return
            }
            alert("远端返回错误：" + String(code.filter { $0.isASCII && ($0.isLetter || $0.isNumber || $0 == "_") }.prefix(80))); return
        }
        if type == "session.started" || type == "session.snapshot" {
            guard let id = object["session_id"] as? String, id.count <= 128 else { return }
            if let session = session, session != id { return }
            session = id; statusItem?.button?.title = "RemoteApp · 已连接"
        }
        guard let currentSession = session, object["session_id"] as? String == currentSession else { return }
        // Media and control have independent arrival order. A UDP frame cannot
        // advance the SSH control sequence and discard an earlier resize/close.
        if !(type == "frame" && object["transport"] as? String == "webrtc") {
            guard let seq = unsignedInteger(object["seq"]) else { transport.disconnect(); return }
            guard seq > lastSequence else { return }; lastSequence = seq
        }
        if type == "clipboard.data" {
            guard let id = object["request_id"] as? String,
                  let number = unsignedInteger(object["window_id"]), number <= UInt32.max,
                  windows[UInt32(number)] != nil, let text = object["text"] as? String,
                  let accepted = clipboardCopyGate.accept(id: id, window: UInt32(number), text: text) else { return }
            NSPasteboard.general.clearContents(); NSPasteboard.general.setString(accepted, forType: .string)
            return
        }
        if type == "session.snapshot" {
            for host in windows.values { host.closeLocally() }; windows.removeAll(); parents.removeAll(); removed.removeAll()
            for item in object["windows"] as? [[String: Any]] ?? [] { updateWindow(item, create: true) }
            attachParents(); return
        }
        if type == "session.exited" {
            for host in windows.values { host.closeLocally() }; windows.removeAll()
            statusItem?.button?.title = "RemoteApp · 应用已退出"; return
        }
        guard let number = unsignedInteger(object["window_id"]), number <= UInt64(UInt32.max) else { return }
        let id = UInt32(number)
        switch type {
        case "window.created": updateWindow(object, create: true); attachParents()
        case "window.metadata": updateWindow(object, create: false); attachParents()
        case "window.destroyed":
            windows.removeValue(forKey: id)?.closeLocally(); parents.removeValue(forKey: id); removed.insert(id)
        case "frame":
            guard !removed.contains(id), let host = windows[id], let image = incoming.image,
                  let revision = unsignedInteger(object["geometry_revision"]), revision == host.revision,
                  image.width == host.surface.remoteWidth, image.height == host.surface.remoteHeight else { return }
            host.surface.image = image
            let video = object["codec"] as? String == "h264"
            let mode = video ? ((object["transport"] as? String == "webrtc" ? "WebRTC" : "SSH") + " · H.264 视频") : (object["video_error"] == nil ? "PNG 图片" : "编码不可用 · PNG 回退")
            if host.mediaMode != mode { host.mediaMode = mode; host.updateSubtitle() }
            statusItem?.button?.title = "RemoteApp · " + mode
        default: break
        }
    }
    func updateWindow(_ object: [String: Any], create: Bool) {
        guard let number = unsignedInteger(object["window_id"]), number <= UInt64(UInt32.max),
              let w = object["width"] as? Int, let h = object["height"] as? Int,
              w > 0, h > 0, w <= 4096, h <= 4096, w * h <= maximumPixels,
              let rev = unsignedInteger(object["geometry_revision"]) else { return }
        let scaleValue = object["scale_factor"] == nil ? 1 : unsignedInteger(object["scale_factor"])
        guard let scale = scaleValue, scale <= 2, let rendering = RenderGeometry(scale: Int(scale)) else { transport.disconnect(); return }
        let id = UInt32(number)
        if removed.contains(id) { return } // XID reuse in one connection is deliberately not supported.
        let otherPixels = windows.filter { $0.key != id }.values.reduce(0) { $0 + $1.surface.remoteWidth * $1.surface.remoteHeight }
        guard otherPixels + w * h <= 16_000_000 else { transport.disconnect(); return }
        let justCreated = windows[id] == nil
        if justCreated {
            guard create, windows.count < 64 else { return }
            let host = HostedWindow(id: id, width: w, height: h, revision: rev, scale: Int(scale),
                popup: object["role"] as? String == "popup") { [weak self] in self?.send($0) }
            windows[id] = host
        }
        guard let host = windows[id], rev >= host.revision else { return }
        host.surface.controllerTextEnabled = capabilities.contains("input.text.commit")
        host.surface.controllerPasteEnabled = capabilities.contains("clipboard.paste")
        host.surface.controllerCopyEnabled = capabilities.contains("clipboard.copy")
        host.revision = rev
        host.window.title = String((object["title"] as? String ?? "Linux 应用").prefix(512))
        host.applyRemoteGeometry(width: w, height: h, rendering: rendering)
        let visible = object["visible"] as? Bool ?? true
        if visible != host.remoteVisible {
            host.remoteVisible = visible
            if visible { host.window.makeKeyAndOrderFront(nil) }
            else { host.surface.releaseInput(); host.window.orderOut(nil) }
        }
        if justCreated, visible, object["role"] as? String == "toplevel" {
            DispatchQueue.main.async { [weak host] in host?.fitToScreen() }
        }
        if let parent = unsignedInteger(object["parent_id"]), parent <= UInt64(UInt32.max) { parents[id] = UInt32(parent) }
        else { parents.removeValue(forKey: id); host.window.parent?.removeChildWindow(host.window) }
    }
    func attachParents() {
        for (id, parentID) in parents where id != parentID {
            guard let child = windows[id]?.window, let parent = windows[parentID]?.window else { continue }
            // Do not permit cycles from malformed window metadata.
            var ancestor: UInt32? = parentID; var seen: Set<UInt32> = [id]; var valid = true
            while let next = ancestor { if !seen.insert(next).inserted { valid = false; break }; ancestor = parents[next] }
            if valid, child.parent !== parent { child.parent?.removeChildWindow(child); parent.addChildWindow(child, ordered: .above) }
        }
    }
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        for host in windows.values { host.surface.releaseInput() }
        send(["type": "session.detach"])
        // An intentional detach can finish SSH before AppKit completes quit.
        // Stop diagnostics now so the polling timer cannot open a modal alert.
        connected = false
        poll?.invalidate(); poll = nil
        // Give the serial write queue a brief opportunity, then close local transport only.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) { [weak self] in
            self?.transport.disconnect(); sender.reply(toApplicationShouldTerminate: true)
        }
        return .terminateLater
    }
    func applicationWillTerminate(_ notification: Notification) { poll?.invalidate(); transport.disconnect() }
}

let application = NSApplication.shared
var applicationDelegate: AppDelegate?
do {
    let options = try Options.parse(Array(CommandLine.arguments.dropFirst()))
    applicationDelegate = AppDelegate(options: options)
    application.delegate = applicationDelegate
    application.run()
} catch HostError.message(let message) {
    FileHandle.standardError.write(Data((message + "\n").utf8)); exit(2)
} catch {
    FileHandle.standardError.write(Data("无法启动 WindowHost\n".utf8)); exit(2)
}
