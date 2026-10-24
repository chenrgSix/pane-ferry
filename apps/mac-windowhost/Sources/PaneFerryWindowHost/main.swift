import AppKit
import Foundation
import Network

// RWP/1 developer preview. Rendering is CoreGraphics/NSView, not Metal.
let maximumLine = 16 * 1024 * 1024
let maximumPixels = 8_000_000
let maximumVideoFPS = 90
func automaticVideoFPS(displayHz: Int, publisherMax: Int = maximumVideoFPS) -> Int {
    let refresh = displayHz > 0 ? displayHz : 60
    return min(refresh, max(1, min(publisherMax, maximumVideoFPS)))
}
func unsignedInteger(_ value: Any?) -> UInt64? {
    guard let number = value as? NSNumber, CFGetTypeID(number) != CFBooleanGetTypeID() else { return nil }
    let string = number.stringValue
    guard !string.isEmpty, string.utf8.allSatisfy({ $0 >= 48 && $0 <= 57 }) else { return nil }
    return UInt64(string)
}

struct Options {
    let app: String?
    let session: String?
    var iconFromStdin = false
    let native: NativeRoute
    static func parse(_ args: [String]) throws -> Options {
        var values: [String: String] = [:]
        var i = 0
        var iconFromStdin = false
        let allowed: Set<String> = ["--app", "--session", "--native-peer", "--native-state", "--native-helper"]
        while i < args.count {
            if args[i] == "--icon-stdin" {
                guard !iconFromStdin else { throw HostError.message("图标参数重复") }
                iconFromStdin = true; i += 1; continue
            }
            guard allowed.contains(args[i]), i + 1 < args.count, values[args[i]] == nil else {
                throw HostError.message("参数格式错误；使用 --native-peer UUID --native-state DIR --native-helper PATH 和 --app APP_ID 或 --session SESSION_ID")
            }
            values[args[i]] = args[i + 1]; i += 2
        }
        guard let peer = values["--native-peer"], UUID(uuidString: peer)?.uuidString.lowercased() == peer,
              let state = values["--native-state"], state.hasPrefix("/"), !state.contains("\0"),
              let helper = values["--native-helper"], helper.hasPrefix("/"), !helper.contains("\0")
        else { throw HostError.message("必须指定已配对设备和本机网络组件。") }
        let native = NativeRoute(peer: peer, state: state, helper: helper)
        let app = values["--app"], session = values["--session"]
        guard (app == nil) != (session == nil) else { throw HostError.message("必须且只能指定 --app 或 --session") }
        for id in [app, session].compactMap({ $0 }) {
            guard id.count <= 128, id.range(of: "^[A-Za-z0-9][A-Za-z0-9_.-]*$", options: .regularExpression) != nil else {
                throw HostError.message("应用/会话 ID 无效")
            }
        }
        return Options(app: app, session: session, iconFromStdin: iconFromStdin, native: native)
    }
}
enum HostError: Error { case message(String) }

struct Incoming {
    let object: [String: Any]
    let image: CGImage?
}
struct TransportFailure {
    let message: String
    let retryable: Bool
}

/// The local QUIC bridge carries the bounded RWP stream and decoder lifecycle.
final class RWPTransport {
    private let process = Process()
    private let input = Pipe()
    private let output = Pipe()
    private let lock = NSLock()
    private var inbox: [Incoming] = []
    private var writes: [(data: Data, moveWindow: UInt32?)] = []
    private var writing = false
    private var closed = false
    private var failure: TransportFailure?
    private var queuedPixels = 0
    private let writer = DispatchQueue(label: "paneferry.rwp.writer")
    var alive: Bool { process.isRunning }
    var onDiagnostic: ((String) -> Void)?

    func start(_ options: Options) throws {
        process.executableURL = URL(fileURLWithPath: options.native.helper)
        process.arguments = ["bridge", "--socket", options.native.socket]
        process.standardInput = input; process.standardOutput = output; process.standardError = FileHandle.nullDevice
        // Do not expose remote stderr (which may contain private app data) in normal logs.
        process.terminationHandler = { [weak self] process in
            self?.fail("设备连接已结束（退出码 \(process.terminationStatus)）。远端会话不会因此主动终止；可用会话 ID 重新连接。")
        }
        try process.run()
        DispatchQueue(label: "paneferry.rwp.reader").async { [weak self] in self?.readLoop() }
    }

    private func fail(_ message: String, retryable: Bool = true) {
        lock.lock(); if failure == nil { failure = TransportFailure(message: message, retryable: retryable) }; lock.unlock()
    }
    func drain() -> ([Incoming], TransportFailure?) {
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
                    let allowedTypes: Set<String> = ["hello", "error", "video.ready", "input.authorization", "session.started", "session.snapshot", "session.exited", "window.created", "window.metadata", "window.destroyed", "frame", "clipboard.data", "clipboard.bundle"]
                    guard allowedTypes.contains(object["type"] as? String ?? "") else { throw HostError.message("未知 RWP 消息类型") }
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
                        guard object["codec"] as? String == "h264",
                              let id = unsignedInteger(object["window_id"]), id <= UInt32.max,
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
                    }
                    var lightweight = object; lightweight.removeValue(forKey: "data")
                    try enqueue(Incoming(object: lightweight, image: image))
                }
                guard buffer.count <= maximumLine else { throw HostError.message("协议消息未终止或过大") }
            }
            // A network interruption may truncate the final frame. Resume from
            // a fresh snapshot instead of treating that as a protocol upgrade.
        } catch { fail("RWP 数据读取失败，已停止接收。请检查版本和连接。", retryable: false) }
        disconnect()
    }
    func send(_ object: [String: Any]) {
        var message = object; message["version"] = 1
        guard let encoded = try? JSONSerialization.data(withJSONObject: message), encoded.count < maximumLine else { return }
        var line = encoded; line.append(10)
        lock.lock()
        guard !closed else { lock.unlock(); return }
        let moveWindow: UInt32? = object["type"] as? String == "input.pointer"
            && object["button"] as? Int == 0 && object["pressed"] as? Bool == false
            ? object["window_id"] as? UInt32 : nil
        if let moveWindow, writes.last?.moveWindow == moveWindow {
            writes[writes.count - 1] = (line, moveWindow)
        } else {
            guard writes.count < 256 else { lock.unlock(); fail("输入队列已满，连接停止，避免输入失序"); disconnect(); return }
            writes.append((line, moveWindow))
        }
        if writing { lock.unlock(); return }
        writing = true; lock.unlock()
        writer.async { [weak self] in self?.writeLoop() }
    }
    private func writeLoop() {
        while true {
            lock.lock()
            if closed || writes.isEmpty { writing = false; lock.unlock(); return }
            let next = writes.removeFirst().data; lock.unlock()
            do { try input.fileHandleForWriting.write(contentsOf: next) }
            catch { fail("向远端发送消息失败"); disconnect(); return }
        }
    }
    func disconnect() {
        lock.lock(); let wasClosed = closed; closed = true; writes.removeAll(); lock.unlock()
        if wasClosed { return }
        try? input.fileHandleForWriting.close()
        try? output.fileHandleForReading.close()
        if process.isRunning { process.terminate() }
    }
}


final class AppDelegate: NSObject, NSApplicationDelegate, NSMenuItemValidation {
    let options: Options
    let applicationName = Bundle.main.object(forInfoDictionaryKey:"CFBundleDisplayName") as? String
        ?? Bundle.main.object(forInfoDictionaryKey:"CFBundleName") as? String ?? "窗渡 PaneFerry"
    let applicationIcon: NSImage?
    var transport = RWPTransport()
    var makeTransport: () -> RWPTransport = { RWPTransport() }
    var windows: [UInt32: HostedWindow] = [:]
    var receivedWindow = false
    var parents: [UInt32: UInt32] = [:]
    var removed = Set<UInt32>()
    var session: String?
    var lastSequence: UInt64 = 0
    var ready = false
    var connected = true
    var recovering = false
    var stopping = false
    var reconnectAttempt = 0
    var reconnectTimer: Timer?
    var handshakeTimer: Timer?
    var networkMonitor: NWPathMonitor?
    let launchRequestID = UUID().uuidString
    var poll: Timer?
    var statusItem: NSStatusItem?
    var rateMenuItem: NSMenuItem?
    var capabilities = Set<String>()
    var videoFPS = 60
    var displayHz = 60
    var publisherMaxVideoFPS = maximumVideoFPS
    var lastVideoFeedback: TimeInterval = 0
    var lastFrameRateRefresh: TimeInterval = 0
    var pendingVideoFeedbackID: UInt64?
    let controllerClipboard: NSPasteboard
    var wireSink: (([String: Any]) -> Void)?
    var terminateHost: () -> Void = { NSApp.terminate(nil) }
    var desktopInputNotice: String?
    private func writeWire(_ message: [String: Any]) { if let sink = wireSink { sink(message) } else { transport.send(message) } }
    var clipboardCopyGate = ClipboardCopyGate()
    var clipboardBusy = false
    var clipboardReading = false
    var clipboardDeferred: [[String: Any]] = []
    var clipboardJob: UUID?
    var clipboardTimer: Timer?
    var clipboardChangeCount = 0
    var bulk: ClipboardBulkTransfer { ClipboardBulkTransfer(native: options.native) }
    init(options: Options, clipboard: NSPasteboard = .general, applicationIcon: NSImage? = nil) {
        self.options = options
        self.applicationIcon = applicationIcon; controllerClipboard = clipboard; super.init()
    }
    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.regular)
        if let icon = applicationIcon { NSApp.applicationIconImage = icon }
        let mainMenu = NSMenu()
        let appItem = NSMenuItem(title:applicationName, action:nil, keyEquivalent:""); mainMenu.addItem(appItem)
        let menu = NSMenu(); appItem.submenu = menu
        menu.addItem(withTitle: "关于 \(applicationName)", action: #selector(showAbout), keyEquivalent: "")
        menu.addItem(withTitle: "复制会话 ID", action: #selector(copySession), keyEquivalent: "")
        menu.addItem(.separator())
        menu.addItem(withTitle: "断开并退出（保留远端应用）", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        NSApp.mainMenu = mainMenu
        let editItem = NSMenuItem(); mainMenu.addItem(editItem)
        let editMenu = NSMenu(title: "编辑"); editItem.submenu = editMenu
        editMenu.addItem(withTitle: "剪切到控制端", action: #selector(SurfaceView.cut(_:)), keyEquivalent: "x")
        editMenu.addItem(withTitle: "复制到控制端", action: #selector(SurfaceView.copy(_:)), keyEquivalent: "c")
        editMenu.addItem(withTitle: "从控制端粘贴", action: #selector(SurfaceView.paste(_:)), keyEquivalent: "v")
        let displayItem = NSMenuItem(); mainMenu.addItem(displayItem)
        let displayMenu = NSMenu(title: "显示"); displayItem.submenu = displayMenu
        displayMenu.addItem(withTitle: "自动适应窗口", action: #selector(automaticResolution), keyEquivalent: "0")
        displayMenu.addItem(withTitle: "适应可用屏幕", action: #selector(fitScreen), keyEquivalent: "")
        displayMenu.addItem(withTitle: "全屏显示", action: #selector(fullScreen), keyEquivalent: "")
        displayMenu.addItem(withTitle: "设置分辨率…", action: #selector(setResolution), keyEquivalent: "")
        displayMenu.addItem(withTitle: "原始像素大小", action: #selector(actualPixels), keyEquivalent: "")
        let rateItem = NSMenuItem(title: "帧率：自动", action: nil, keyEquivalent: "")
        rateItem.isEnabled = false; rateMenuItem = rateItem; displayMenu.addItem(rateItem)
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        statusItem?.button?.title = "PaneFerry · 连接中"; statusItem?.menu = menu.copy() as? NSMenu
        startConnection()
        let timer = Timer(timeInterval: 1.0 / 90.0, repeats: true) { [weak self] _ in self?.pump() }
        RunLoop.main.add(timer, forMode: .common); poll = timer
        let monitor = NWPathMonitor(); networkMonitor = monitor
        monitor.pathUpdateHandler = { [weak self] path in
            DispatchQueue.main.async {
                guard let self = self, !self.stopping else { return }
                if path.status == .satisfied, self.recovering, !self.connected { self.startConnection() }
                else if path.status == .unsatisfied, self.connected { self.connectionLost() }
            }
        }
        monitor.start(queue: DispatchQueue(label: "paneferry.network"))
        NotificationCenter.default.addObserver(self, selector: #selector(screenParametersChanged),
                                               name: NSApplication.didChangeScreenParametersNotification, object: nil)
        NSApp.activate(ignoringOtherApps: true)
    }
    @objc func showAbout() {
        alert("窗渡 PaneFerry 开发预览\n远端窗口跟随本机内容区的逻辑大小，已有应用沿用原缩放；收到的 H.264 画面等比显示。帧率按本机显示器与远端能力自动协商，协议最高 90 fps，并随画面反馈调整。新应用按本机显示倍率请求 100% 或 200% 启动缩放，实际效果取决于应用。输入法在控制端组合，剪贴板只由主动操作触发。")
    }
    @objc func screenParametersChanged(_ notification: Notification) { refreshVideoFPS() }
    func updateRateMenu() {
        rateMenuItem?.title = !ready ? "帧率：自动协商中" : capabilities.contains("video.qos")
            ? "帧率：自动 · 屏幕 \(displayHz) Hz · 上限 \(videoFPS) fps"
            : "帧率：远端固定 60 fps"
    }
    func refreshVideoFPS(displayRefresh: Int? = nil) {
        let screens = windows.values.filter { $0.remoteVisible }.compactMap { $0.window.screen }
        let refresh = displayRefresh ?? (screens.isEmpty ? NSScreen.main?.maximumFramesPerSecond : screens.map(\.maximumFramesPerSecond).max()) ?? 60
        displayHz = refresh > 0 ? refresh : 60
        let selected = automaticVideoFPS(displayHz: displayHz, publisherMax: publisherMaxVideoFPS)
        updateRateMenu()
        guard selected != videoFPS else { return }
        videoFPS = selected
        updateRateMenu()
        if ready && capabilities.contains("video.qos") { send(["type": "video.configure", "max_fps": selected]) }
    }
    func activeHost() -> HostedWindow? { windows.values.first { $0.window === NSApp.keyWindow } }
    @objc func automaticResolution() { activeHost()?.useAutomaticResolution() }
    @objc func fitScreen() { activeHost()?.fitToScreen() }
    @objc func actualPixels() { activeHost()?.showActualPixels() }
    @objc func fullScreen() { activeHost()?.window.toggleFullScreen(nil) }
    func validateMenuItem(_ menuItem: NSMenuItem) -> Bool {
        if menuItem.action == #selector(setResolution) { return activeHost()?.desktopMode != true }
        if menuItem.action == #selector(automaticResolution) {
            menuItem.state = activeHost()?.fixedPixels == nil ? .on : .off
            return activeHost() != nil
        }
        return true
    }
    @objc func setResolution() {
        guard let host = activeHost() else { return }
        if host.desktopMode { return }
        let dialog = NSAlert(); dialog.messageText = "设置远端窗口分辨率"
        dialog.informativeText = "输入远端实际渲染像素宽高，固定模式按比例显示。选择“自动适应窗口”可恢复跟随本机逻辑大小，保留应用现有缩放。上限 4096×2160、800 万像素。"
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
        controllerClipboard.clearContents(); controllerClipboard.setString(session, forType: .string)
    }
    private func alert(_ message: String) {
        let alert = NSAlert(); alert.messageText = applicationName; alert.informativeText = message
        if let icon = applicationIcon { alert.icon = icon }
        alert.addButton(withTitle: "好"); alert.runModal()
    }
    func pump() {
        let (events, error) = transport.drain()
        for event in events { if !connected { break }; receive(event) }
        if let error = error, connected {
            if error.retryable { connectionLost() }
            else { stopRecovery(); alert(error.message) }
        }
        if let id = pendingVideoFeedbackID, connected {
            let now = ProcessInfo.processInfo.systemUptime
            if lastVideoFeedback == 0 || now - lastVideoFeedback >= 0.05 {
                pendingVideoFeedbackID = nil
                lastVideoFeedback = now
                send(["type": "video.feedback", "frame_id": id])
            }
        }
        let now = ProcessInfo.processInfo.systemUptime
        if now - lastFrameRateRefresh >= 1 {
            lastFrameRateRefresh = now
            for host in windows.values { host.updateSubtitle() }
        }
    }
    func startConnection() {
        guard !stopping else { return }
        reconnectTimer?.invalidate(); reconnectTimer = nil
        transport = makeTransport(); connected = true; ready = false; lastSequence = 0; capabilities.removeAll()
        publisherMaxVideoFPS = maximumVideoFPS
        updateRateMenu()
        lastVideoFeedback = 0; pendingVideoFeedbackID = nil
        do {
            try transport.start(options)
            writeWire(["type": "hello"])
            let current = transport
            let timer = Timer(timeInterval: 15, repeats: false) { [weak self, weak current] _ in
                guard let self = self, self.transport === current, !self.stopping else { return }
                self.connectionLost()
            }
            handshakeTimer?.invalidate(); handshakeTimer = timer
            RunLoop.main.add(timer, forMode: .common)
        } catch { connectionLost() }
    }
    static func reconnectDelay(_ attempt: Int) -> TimeInterval { [1, 2, 4, 8, 15][min(max(0, attempt), 4)] }
    func pauseConnection() {
        connected = false; ready = false; transport.disconnect()
        handshakeTimer?.invalidate(); handshakeTimer = nil
        clipboardJob = nil; clipboardTimer?.invalidate(); clipboardTimer = nil
        clipboardDeferred.removeAll(); clipboardBusy = false; clipboardReading = false; clipboardCopyGate = ClipboardCopyGate()
        for host in windows.values {
            host.surface.transmit = nil; host.surface.releaseInput(); host.surface.unmarkText()
            host.resizeTimer?.invalidate(); host.resizeTimer = nil
        }
    }
    func connectionLost() {
        guard !stopping else { return }
        pauseConnection(); recovering = true
        guard reconnectTimer == nil else { return }
        for host in windows.values { host.window.subtitle = "连接中断，正在自动重连…" }
        statusItem?.button?.title = "PaneFerry · 正在重连"
        let delay = Self.reconnectDelay(reconnectAttempt); reconnectAttempt += 1
        let timer = Timer(timeInterval: delay, repeats: false) { [weak self] _ in self?.startConnection() }
        reconnectTimer = timer; RunLoop.main.add(timer, forMode: .common)
    }
    func stopRecovery() {
        pauseConnection(); recovering = false
        reconnectTimer?.invalidate(); reconnectTimer = nil
        statusItem?.button?.title = "PaneFerry · 已断开"
        for host in windows.values { host.window.subtitle = "已断开，请从启动器重新连接。" }
    }
    func finishSession() {
        guard !stopping else { return }
        stopping = true
        stopRecovery()
        poll?.invalidate(); poll = nil
        networkMonitor?.cancel(); networkMonitor = nil
        for host in windows.values { host.closeLocally() }
        windows.removeAll(); parents.removeAll(); removed.removeAll()
        session = nil
        terminateHost()
    }
    private func beginClipboard(timeout: TimeInterval, reading: Bool = false) -> UUID {
        clipboardBusy = true; clipboardReading = reading
        let job = UUID(); clipboardJob = job; clipboardChangeCount = controllerClipboard.changeCount
        clipboardTimer?.invalidate()
        clipboardTimer = Timer.scheduledTimer(withTimeInterval: timeout, repeats: false) { [weak self] _ in
            guard let self = self, self.clipboardJob == job else { return }; self.clipboardFailed()
        }
        return job
    }
    private func finishClipboard() {
        clipboardJob = nil; clipboardTimer?.invalidate(); clipboardTimer = nil; clipboardBusy = false; clipboardReading = false
        if connected { statusItem?.button?.title = "PaneFerry · 已连接" }
        let queued = clipboardDeferred; clipboardDeferred.removeAll()
        for message in queued { send(message) }
    }
    private func clipboardFailed() {
        if clipboardReading {
            var replacement = false
            clipboardDeferred = clipboardDeferred.filter { message in
                let type = message["type"] as? String ?? ""
                if ["clipboard.copy", "clipboard.cut"].contains(type) { replacement = true }
                return type != "controller.paste" || replacement
            }
        }
        NSSound.beep(); finishClipboard(); statusItem?.button?.title = "PaneFerry · 剪贴板操作失败"
    }
    private func pasteController(_ window: UInt32) {
        do {
            guard capabilities.contains("clipboard.paste") else { throw ClipboardFailure.unsupported }
            guard let payload = try ControllerClipboard.read(controllerClipboard) else { throw ClipboardFailure.unsupported }
            switch payload {
            case .text(let text): writeWire(["type":"clipboard.paste", "window_id":window, "text":text])
            case .bundle(let payload):
                guard capabilities.contains("clipboard.bundle") else { throw ClipboardFailure.unsupported }
                let job = beginClipboard(timeout: 310); let transfer = bulk
                statusItem?.button?.title = "PaneFerry · 正在传输剪贴板"
                DispatchQueue.global(qos: .userInitiated).async { [weak self] in
                    let result = Result { try transfer.upload(payload) }
                    DispatchQueue.main.async {
                        guard let self = self, self.connected, self.clipboardJob == job, self.windows[window] != nil else { return }
                        switch result {
                        case .success(let id): self.writeWire(["type":"clipboard.paste.bundle", "window_id":window, "transfer_id":id]); self.finishClipboard()
                        case .failure: self.clipboardFailed()
                        }
                    }
                }
            }
        } catch { clipboardFailed() }
    }
    func send(_ message: [String: Any]) {
        guard connected else { return }
        let type = message["type"] as? String ?? ""
        if recovering && !["video.enable", "session.resume", "app.launch"].contains(type) { return }
        if type == "input.pointer", message["pressed"] as? Bool == true, (message["button"] as? Int ?? 0) > 0,
           let window = message["window_id"] as? UInt32 {
            windows[window]?.inputNotice = desktopInputNotice
        }
        if clipboardBusy && type != "session.detach" && type != "video.feedback" {
            if type == "input.pointer", message["button"] as? Int == 0,
               clipboardDeferred.last?["type"] as? String == type, clipboardDeferred.last?["button"] as? Int == 0,
               clipboardDeferred.last?["window_id"] as? UInt32 == message["window_id"] as? UInt32 { clipboardDeferred.removeLast() }
            guard clipboardDeferred.count < 256 else { transport.disconnect(); return }
            clipboardDeferred.append(message); return
        }
        if type == "controller.paste", let window = message["window_id"] as? UInt32 { pasteController(window); return }
        var wire = message
        if ["input.text.commit", "clipboard.paste", "clipboard.copy", "clipboard.cut"].contains(type) {
            guard capabilities.contains(type) else { NSSound.beep(); return }
            if ["clipboard.copy", "clipboard.cut"].contains(type), let id = message["request_id"] as? String,
               let window = message["window_id"] as? UInt32 {
                let rich = capabilities.contains("clipboard.bundle")
                _ = beginClipboard(timeout: rich ? 35 : 5, reading: true)
                clipboardCopyGate.begin(id: id, window: window, timeout: rich ? 35 : 5)
                if rich { wire["type"] = "clipboard.copy.bundle"; wire["cut"] = type == "clipboard.cut" }
            }
        }
        // One attached session; bulk bytes travel in an independent transport stream.
        writeWire(wire)
    }
    func receive(_ incoming: Incoming) {
        let object = incoming.object
        guard let type = object["type"] as? String else { return }
        if type == "hello" {
            guard !ready else { return }
            capabilities = Set(object["capabilities"] as? [String] ?? [])
            guard capabilities.contains("native.h264") else {
                stopRecovery(); alert("远端不支持 H.264 视频，无法连接该窗口。")
                return
            }
            if let limit = unsignedInteger(object["video_max_fps"]), (1...UInt64(maximumVideoFPS)).contains(limit) {
                publisherMaxVideoFPS = Int(limit)
            }
            refreshVideoFPS()
            ready = true
            updateRateMenu()
            var enable: [String: Any] = ["type":"video.enable", "codec":"h264"]
            if capabilities.contains("video.qos") {
                enable["max_fps"] = videoFPS
                enable["feedback"] = true
            }
            send(enable)
            if let id = session ?? options.session { send(["type": "session.resume", "session_id": id]) }
            else if let app = options.app {
                var launch: [String: Any] = ["type": "app.launch", "app_id": app, "request_id": launchRequestID, "backend": "native"]
                if capabilities.contains("display.scale") {
                    launch["scale_factor"] = (NSScreen.main?.backingScaleFactor ?? 1) >= 1.5 ? 2 : 1
                }
                send(launch)
            }
            return
        }
        if type == "video.ready" { statusItem?.button?.title = "PaneFerry · H.264 视频"; return }
        if type == "error" {
            // Display server error code, not an arbitrary message potentially containing app data.
            let code = object["code"] as? String ?? "unknown"
            if code == "resolution_limited" {
                if let number = unsignedInteger(object["window_id"]), number <= UInt32.max,
                   let host = windows[UInt32(number)] {
                    host.resolutionNotice = "超出远端采集范围，请缩小窗口或降低分辨率"
                } else {
                    stopRecovery(); alert("原窗口超过 Wayland 采集范围。请在 Ubuntu 缩小原窗口后重新连接。")
                }
                return
            }
            if ["wayland_binary_clipboard_unavailable", "wayland_cut_unavailable"].contains(code) {
                clipboardFailed()
                alert(code == "wayland_cut_unavailable" ? "当前 Wayland 窗口暂不支持远程剪切，请使用复制；远端内容未删除。" : "当前 Wayland 窗口仅支持文字剪贴板。")
                return
            }
            if code == "wayland_bridge_outdated" {
                stopRecovery(); alert("Ubuntu 仍在运行旧版窗口扩展。保存远端工作后，注销并重新登录以启用新版；当前应用不会自动关闭。"); return
            }
            if ["wayland_bridge_unavailable", "wayland_bridge_failed", "wayland_capture_unavailable", "desktop_locked"].contains(code) {
                stopRecovery(); alert(code == "desktop_locked" ? "远端桌面已锁定，请解锁后重新连接。" : "Wayland 窗口连接未就绪。请确认 Ubuntu 已启用窗渡 GNOME 扩展，且 PipeWire 组件正常。")
                return
            }
            if code == "desktop_secrets_unavailable" {
                stopRecovery()
                alert("应用需要 Ubuntu 桌面密钥服务。请确认 Ubuntu 桌面已登录且已安装 xdg-dbus-proxy，再重新打开应用。")
                return
            }
            if ["desktop_authorization_pending", "desktop_authorization_denied", "desktop_input_paused", "desktop_input_unavailable"].contains(code) {
                // The authorization state event explains the action without a
                // blocking alert for every key pressed while consent is pending.
                if code != "desktop_authorization_pending" { NSSound.beep() }
                return
            }
            if code == "window_unavailable" { return }
            if code == "session_controlled", recovering { connectionLost(); return }
            if ["session_expired", "session_exited"].contains(code) {
                finishSession(); return
            }
            if code == "wayland_text_unavailable" {
                // Missing text-input focus is a capability limit, not a lost
                // connection. Never open one blocking alert per typed character.
                if let number = unsignedInteger(object["window_id"]), number <= UInt32.max,
                   let host = windows[UInt32(number)] {
                    host.inputNotice = "当前输入框不支持直接输入，请使用粘贴"
                }
                statusItem?.button?.title = "PaneFerry · 请使用粘贴输入"; NSSound.beep(); return
            }
            if ["controller_text_failed", "clipboard_copy_failed", "clipboard_cut_failed", "clipboard_cut_unavailable"].contains(code) {
                statusItem?.button?.title = "PaneFerry · 输入或剪贴板失败"
                if code == "controller_text_failed" { NSSound.beep() } else { clipboardFailed() }; return
            }
            if handshakeTimer != nil { stopRecovery() }
            alert("远端返回错误：" + String(code.filter { $0.isASCII && ($0.isLetter || $0.isNumber || $0 == "_") }.prefix(80))); return
        }
        if type == "session.started" || type == "session.snapshot" {
            guard let id = object["session_id"] as? String, id.count <= 128 else { return }
            if let session = session, session != id { return }
            session = id; recovering = false; reconnectAttempt = 0
            reconnectTimer?.invalidate(); reconnectTimer = nil; handshakeTimer?.invalidate(); handshakeTimer = nil
            statusItem?.button?.title = "PaneFerry · 已连接"
        }
        guard let currentSession = session, object["session_id"] as? String == currentSession else { return }
        guard let seq = unsignedInteger(object["seq"]) else { transport.disconnect(); return }
        guard seq > lastSequence else { return }; lastSequence = seq
        if type == "input.authorization" {
            let state = object["state"] as? String ?? "unavailable"
            switch state {
            case "ready": desktopInputNotice = nil
            case "pending": desktopInputNotice = "请在 Ubuntu 确认 窗渡 PaneFerry Agent 控制授权，并记住本次选择"
            case "denied": desktopInputNotice = "Ubuntu 已取消控制授权；重新连接可再次申请"
            case "paused": desktopInputNotice = "Ubuntu 暂停了输入控制，请检查远端锁屏状态"
            default: desktopInputNotice = "Ubuntu 输入授权连接不可用，请重新连接"
            }
            for host in windows.values { host.inputNotice = desktopInputNotice }
            statusItem?.button?.title = desktopInputNotice == nil ? "PaneFerry · 已连接" : "PaneFerry · 等待 Ubuntu 授权"
            return
        }
        if type == "clipboard.data" {
            guard let id = object["request_id"] as? String,
                  let number = unsignedInteger(object["window_id"]), number <= UInt32.max,
                  windows[UInt32(number)] != nil, let text = object["text"] as? String,
                  let accepted = clipboardCopyGate.accept(id: id, window: UInt32(number), text: text) else { return }
            if controllerClipboard.changeCount == clipboardChangeCount {
                controllerClipboard.clearContents(); controllerClipboard.setString(accepted, forType: .string)
            }
            finishClipboard(); return
        }
        if type == "clipboard.bundle" {
            guard let id = object["request_id"] as? String,
                  let number = unsignedInteger(object["window_id"]), number <= UInt32.max, windows[UInt32(number)] != nil,
                  let transferID = object["transfer_id"] as? String, ClipboardBulkTransfer.validID(transferID),
                  clipboardCopyGate.claim(id: id, window: UInt32(number)), let job = clipboardJob else { return }
            clipboardTimer?.invalidate()
            clipboardTimer = Timer.scheduledTimer(withTimeInterval: 310, repeats: false) { [weak self] _ in
                guard let self = self, self.clipboardJob == job else { return }; self.clipboardFailed()
            }
            let transfer = bulk
            let root = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0].appendingPathComponent("LAN RemoteApp/Clipboard")
            let urls = controllerClipboard.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL] ?? []
            let pins = Set(urls.filter { $0.path.hasPrefix(root.path + "/") }.compactMap { $0.path.dropFirst(root.path.count + 1).split(separator: "/").first.map(String.init) })
            statusItem?.button?.title = "PaneFerry · 正在接收剪贴板"
            DispatchQueue.global(qos: .userInitiated).async { [weak self] in
                let result = Result { try transfer.download(transferID, into: root, protecting: pins) }
                DispatchQueue.main.async {
                    guard let self = self, self.connected, self.clipboardJob == job, self.windows[UInt32(number)] != nil else { return }
                    do {
                        let (directory, manifest) = try result.get()
                        if self.controllerClipboard.changeCount == self.clipboardChangeCount { try ControllerClipboard.write(directory, manifest: manifest, to: self.controllerClipboard) }
                        self.finishClipboard()
                    } catch { self.clipboardFailed() }
                }
            }
            return
        }
        if type == "session.snapshot" {
            let snapshot = object["windows"] as? [[String: Any]] ?? []
            let retained = Set(snapshot.compactMap { unsignedInteger($0["window_id"]).flatMap { $0 <= UInt32.max ? UInt32($0) : nil } })
            for id in Array(windows.keys) where !retained.contains(id) { windows.removeValue(forKey: id)?.closeLocally() }
            parents.removeAll(); removed.removeAll()
            for item in snapshot { updateWindow(item, create: true) }
            attachParents()
            for host in windows.values where !host.popup { host.windowDidResize(Notification(name: NSWindow.didResizeNotification)) }
            if let host = activeHost(), host.remoteVisible { send(["type": "window.focus", "window_id": host.id]) }
            return
        }
        if type == "session.exited" {
            if !receivedWindow { alert("应用在产生窗口前已退出，可能已转交给原桌面的实例。请刷新启动器的运行窗口列表后连接已有窗口。") }
            finishSession(); return
        }
        guard let number = unsignedInteger(object["window_id"]), number <= UInt64(UInt32.max) else { return }
        let id = UInt32(number)
        switch type {
        case "window.created": updateWindow(object, create: true); attachParents()
        case "window.metadata": updateWindow(object, create: false); attachParents()
        case "window.destroyed":
            windows.removeValue(forKey: id)?.closeLocally(); parents.removeValue(forKey: id); removed.insert(id)
            refreshVideoFPS()
        case "frame":
            guard !removed.contains(id), let host = windows[id], let image = incoming.image,
                  let revision = unsignedInteger(object["geometry_revision"]), revision == host.revision,
                  image.width == host.surface.remoteWidth, image.height == host.surface.remoteHeight else { return }
            host.present(image, mediaMode: "H.264 视频")
            statusItem?.button?.title = "PaneFerry · H.264 视频"
            if capabilities.contains("video.qos"), let frameID = unsignedInteger(object["frame_id"]), frameID > 0 {
                pendingVideoFeedbackID = frameID
            }
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
            receivedWindow = true
            let host = HostedWindow(id: id, width: w, height: h, revision: rev, scale: Int(scale),
                popup: object["role"] as? String == "popup", disconnect: { NSApp.terminate(nil) }) { [weak self] in self?.send($0) }
            host.desktopMode = object["surface_type"] as? String == "desktop"
            windows[id] = host
            host.inputNotice = desktopInputNotice
            host.onScreenChange = { [weak self] in self?.refreshVideoFPS() }
        }
        guard let host = windows[id], rev >= host.revision else { return }
        host.surface.transmit = { [weak self] in self?.send($0) }
        host.surface.requestControllerPaste = { [weak self] in self?.send(["type":"controller.paste", "window_id":id]) }
        host.surface.controllerTextEnabled = capabilities.contains("input.text.commit")
        host.surface.controllerPasteEnabled = capabilities.contains("clipboard.paste")
        host.surface.controllerCopyEnabled = capabilities.contains("clipboard.copy")
        host.surface.controllerCutEnabled = capabilities.contains("clipboard.cut")
        host.revision = rev
        host.scaleMode = object["scale_mode"] as? String ?? "unknown"
        host.window.title = String((object["title"] as? String ?? "Linux 应用").prefix(512))
        host.applyRemoteGeometry(width: w, height: h, rendering: rendering)
        if let parent = unsignedInteger(object["parent_id"]), parent <= UInt64(UInt32.max), parent != id { parents[id] = UInt32(parent); host.relatedWindow = true }
        else { parents.removeValue(forKey: id); host.relatedWindow = false; host.window.parent?.removeChildWindow(host.window) }
        let visible = object["visible"] as? Bool ?? true
        let visibilityChanged = visible != host.remoteVisible
        host.applyRemoteVisibility(visible)
        if visibilityChanged { refreshVideoFPS() }
        if justCreated, visible, ["toplevel", "desktop"].contains(object["role"] as? String ?? "") {
            DispatchQueue.main.async { [weak host] in host?.fitToScreen() }
        }
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
        // An intentional detach can finish the bridge before AppKit completes quit.
        // Stop diagnostics now so the polling timer cannot open a modal alert.
        stopping = true; reconnectTimer?.invalidate(); reconnectTimer = nil; handshakeTimer?.invalidate(); handshakeTimer = nil
        networkMonitor?.cancel(); networkMonitor = nil
        connected = false; clipboardJob = nil; clipboardTimer?.invalidate(); clipboardDeferred.removeAll()
        poll?.invalidate(); poll = nil
        // Give the serial write queue a brief opportunity, then close local transport only.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) { [weak self] in
            self?.transport.disconnect(); sender.reply(toApplicationShouldTerminate: true)
        }
        return .terminateLater
    }
    func applicationWillTerminate(_ notification: Notification) { stopping = true; NotificationCenter.default.removeObserver(self); networkMonitor?.cancel(); reconnectTimer?.invalidate(); handshakeTimer?.invalidate(); poll?.invalidate(); transport.disconnect() }
}

// WINDOWHOST_ENTRYPOINT
let application = NSApplication.shared
var applicationDelegate: AppDelegate?
do {
    let options = try Options.parse(Array(CommandLine.arguments.dropFirst()))
    let icon = options.iconFromStdin ? ApplicationIcon.read(from: .standardInput) : nil
    applicationDelegate = AppDelegate(options: options, applicationIcon: icon ?? ApplicationIcon.fallback())
    application.delegate = applicationDelegate
    application.run()
} catch HostError.message(let message) {
    FileHandle.standardError.write(Data((message + "\n").utf8)); exit(2)
} catch {
    FileHandle.standardError.write(Data("无法启动 WindowHost\n".utf8)); exit(2)
}
