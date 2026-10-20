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
    static func parse(_ args: [String]) throws -> Options {
        var values: [String: String] = [:]
        var i = 0
        let allowed: Set<String> = ["--host", "--user", "--port", "--app", "--session"]
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
        return Options(host: host, user: user, port: port, app: app, session: session)
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
    private var queuedPixels = 0
    private let writer = DispatchQueue(label: "remoteapp.ssh.writer")
    private(set) var alive = false
    var onDiagnostic: ((String) -> Void)?

    func start(_ options: Options) throws {
        process.executableURL = URL(fileURLWithPath: "/usr/bin/ssh")
        // Ignore SSH config: otherwise a user's ProxyCommand/RemoteCommand could change transport semantics.
        process.arguments = ["-F", "/dev/null", "-T", "-oBatchMode=yes", "-oStrictHostKeyChecking=yes",
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
            while true { do { if try self.errors.fileHandleForReading.read(upToCount: 4096)?.isEmpty != false { break } } catch { break } }
        }
        DispatchQueue(label: "remoteapp.ssh.reader").async { [weak self] in self?.readLoop() }
    }

    private func fail(_ message: String) {
        lock.lock(); if failure == nil { failure = message }; lock.unlock()
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
        do {
            while true {
                lock.lock(); let stop = closed; lock.unlock(); if stop { break }
                guard let chunk = try output.fileHandleForReading.read(upToCount: 65536), !chunk.isEmpty else { break }
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
                    let allowedTypes: Set<String> = ["hello", "error", "session.started", "session.snapshot", "session.exited", "window.created", "window.metadata", "window.destroyed", "frame"]
                    guard allowedTypes.contains(object["type"] as? String ?? "") else { throw HostError.message("未知 RWP 消息类型") }
                    var image: CGImage?
                    if object["type"] as? String == "frame" {
                        guard let w = object["width"] as? Int, let h = object["height"] as? Int,
                              w > 0, h > 0, w <= 4096, h <= 4096, w * h <= maximumPixels,
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
                    var lightweight = object; lightweight.removeValue(forKey: "png")
                    try enqueue(Incoming(object: lightweight, image: image))
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
        lock.lock(); let wasClosed = closed; closed = true; writes.removeAll(); lock.unlock()
        if wasClosed { return }
        try? input.fileHandleForWriting.close()
        try? output.fileHandleForReading.close()
        if process.isRunning { process.terminate() }
    }
}

final class SurfaceView: NSView {
    var remoteID: UInt32 = 0
    var remoteWidth = 1
    var remoteHeight = 1
    var image: CGImage? { didSet { needsDisplay = true } }
    var transmit: (([String: Any]) -> Void)?
    private var keys: [UInt16: UInt32] = [:]
    private var heldModifiers: Set<UInt32> = []
    private var heldButtons: Set<Int> = []
    private var lastPointer = (x: 0, y: 0)
    private var tracking: NSTrackingArea?
    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { true }
    override func draw(_ dirtyRect: NSRect) {
        NSColor.black.setFill(); bounds.fill()
        guard let image = image else { return }
        NSImage(cgImage: image, size: NSSize(width: CGFloat(image.width), height: CGFloat(image.height)))
            .draw(in: bounds, from: .zero, operation: .copy, fraction: 1, respectFlipped: true, hints: [.interpolation: NSImageInterpolation.high])
    }
    override func updateTrackingAreas() {
        if let tracking = tracking { removeTrackingArea(tracking) }
        tracking = NSTrackingArea(rect: .zero, options: [.activeInKeyWindow, .mouseMoved, .inVisibleRect], owner: self, userInfo: nil)
        addTrackingArea(tracking!); super.updateTrackingAreas()
    }
    private func pointer(_ event: NSEvent, button: Int, pressed: Bool) {
        let point = convert(event.locationInWindow, from: nil)
        let x = max(0, min(remoteWidth - 1, Int(point.x / max(bounds.width, 1) * CGFloat(remoteWidth))))
        let y = max(0, min(remoteHeight - 1, Int(point.y / max(bounds.height, 1) * CGFloat(remoteHeight))))
        lastPointer = (x, y)
        if button > 0 { if pressed { heldButtons.insert(button) } else { heldButtons.remove(button) } }
        transmit?(["type": "input.pointer", "window_id": remoteID, "x": x, "y": y, "button": button, "pressed": pressed])
    }
    override func mouseDown(with event: NSEvent) { window?.makeFirstResponder(self); pointer(event, button: 1, pressed: true) }
    override func mouseUp(with event: NSEvent) { pointer(event, button: 1, pressed: false) }
    override func rightMouseDown(with event: NSEvent) { pointer(event, button: 3, pressed: true) }
    override func rightMouseUp(with event: NSEvent) { pointer(event, button: 3, pressed: false) }
    override func otherMouseDown(with event: NSEvent) { pointer(event, button: 2, pressed: true) }
    override func otherMouseUp(with event: NSEvent) { pointer(event, button: 2, pressed: false) }
    override func mouseMoved(with event: NSEvent) { pointer(event, button: 0, pressed: false) }
    override func mouseDragged(with event: NSEvent) { pointer(event, button: 0, pressed: false) }
    override func rightMouseDragged(with event: NSEvent) { pointer(event, button: 0, pressed: false) }
    override func otherMouseDragged(with event: NSEvent) { pointer(event, button: 0, pressed: false) }
    override func scrollWheel(with event: NSEvent) {
        let vertical = event.scrollingDeltaY
        let horizontal = event.scrollingDeltaX
        let value = abs(vertical) >= abs(horizontal) ? vertical : horizontal
        guard value != 0 else { return }
        let button = abs(vertical) >= abs(horizontal) ? (value > 0 ? 4 : 5) : (value > 0 ? 6 : 7)
        for _ in 0..<min(5, max(1, Int(abs(value) / (event.hasPreciseScrollingDeltas ? 10 : 1)))) {
            pointer(event, button: button, pressed: true); pointer(event, button: button, pressed: false)
        }
    }
    private func key(_ symbol: UInt32, _ down: Bool) { transmit?(["type": "input.key", "window_id": remoteID, "keysym": symbol, "pressed": down]) }
    override func keyDown(with event: NSEvent) {
        syncModifiers(event.modifierFlags)
        guard let symbol = keys[event.keyCode] ?? Self.keysym(event) else { NSSound.beep(); return }
        keys[event.keyCode] = symbol; key(symbol, true)
    }
    override func keyUp(with event: NSEvent) {
        if let symbol = keys.removeValue(forKey: event.keyCode) { key(symbol, false) }
        syncModifiers(event.modifierFlags)
    }
    override func flagsChanged(with event: NSEvent) { syncModifiers(event.modifierFlags) }
    private func syncModifiers(_ flags: NSEvent.ModifierFlags) {
        var target = Set<UInt32>()
        if flags.contains(.shift) { target.insert(0xffe1) }
        if flags.contains(.control) || flags.contains(.command) { target.insert(0xffe3) }
        if flags.contains(.option) { target.insert(0xffe9) }
        for symbol in heldModifiers.subtracting(target) { key(symbol, false) }
        for symbol in target.subtracting(heldModifiers) { key(symbol, true) }
        heldModifiers = target
    }
    func releaseInput() {
        for button in heldButtons { transmit?(["type": "input.pointer", "window_id": remoteID, "x": lastPointer.x, "y": lastPointer.y, "button": button, "pressed": false]) }
        heldButtons.removeAll()
        for symbol in Set(keys.values) { key(symbol, false) }; keys.removeAll()
        for symbol in heldModifiers { key(symbol, false) }; heldModifiers.removeAll()
    }
    static func keysym(_ event: NSEvent) -> UInt32? {
        let special: [UInt16: UInt32] = [36: 0xff0d, 48: 0xff09, 51: 0xff08, 53: 0xff1b, 117: 0xffff,
            123: 0xff51, 124: 0xff53, 125: 0xff54, 126: 0xff52, 115: 0xff50, 119: 0xff57,
            116: 0xff55, 121: 0xff56, 76: 0xff8d,
            122: 0xffbe, 120: 0xffbf, 99: 0xffc0, 118: 0xffc1, 96: 0xffc2, 97: 0xffc3,
            98: 0xffc4, 100: 0xffc5, 101: 0xffc6, 109: 0xffc7, 103: 0xffc8, 111: 0xffc9]
        if let symbol = special[event.keyCode] { return symbol }
        // Physical ANSI fallback deliberately avoids claiming Unicode/IME support.
        let ansi: [UInt16: String] = [0:"a",1:"s",2:"d",3:"f",4:"h",5:"g",6:"z",7:"x",8:"c",9:"v",11:"b",
            12:"q",13:"w",14:"e",15:"r",16:"y",17:"t",18:"1",19:"2",20:"3",21:"4",22:"6",23:"5",24:"=",25:"9",26:"7",27:"-",28:"8",29:"0",
            30:"]",31:"o",32:"u",33:"[",34:"i",35:"p",37:"l",38:"j",39:"'",40:"k",41:";",42:"\\",43:",",44:"/",45:"n",46:"m",47:".",49:" ",50:"`"]
        return ansi[event.keyCode]?.unicodeScalars.first?.value
    }
}

final class HostedWindow: NSObject, NSWindowDelegate {
    let id: UInt32
    let window: NSWindow
    let surface = SurfaceView(frame: .zero)
    var revision: UInt64
    var applyingRemote = false
    var allowClose = false
    var resizeTimer: Timer?
    var transmit: ([String: Any]) -> Void
    init(id: UInt32, width: Int, height: Int, revision: UInt64, popup: Bool, transmit: @escaping ([String: Any]) -> Void) {
        self.id = id; self.revision = revision; self.transmit = transmit
        let style: NSWindow.StyleMask = popup ? [.borderless] : [.titled, .closable, .miniaturizable, .resizable]
        self.window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: CGFloat(width), height: CGFloat(height)), styleMask: style, backing: .buffered, defer: false)
        super.init()
        window.delegate = self; window.isReleasedWhenClosed = false
        window.contentMinSize = NSSize(width: 64, height: 48)
        window.contentMaxSize = NSSize(width: 4096, height: 4096)
        window.acceptsMouseMovedEvents = true
        surface.remoteID = id; surface.remoteWidth = width; surface.remoteHeight = height; surface.transmit = transmit
        window.contentView = surface; window.center()
        window.makeFirstResponder(surface)
    }
    func windowShouldClose(_ sender: NSWindow) -> Bool {
        if allowClose { return true }
        transmit(["type": "window.close.request", "window_id": id]); return false
    }
    func windowDidBecomeKey(_ notification: Notification) { transmit(["type": "window.focus", "window_id": id]) }
    func windowDidResignKey(_ notification: Notification) { surface.releaseInput() }
    func windowDidResize(_ notification: Notification) {
        guard !applyingRemote else { return }
        resizeTimer?.invalidate()
        resizeTimer = Timer.scheduledTimer(withTimeInterval: 0.10, repeats: false) { [weak self] _ in
            guard let self = self else { return }
            let size = self.surface.bounds.size
            let w = min(4096, max(64, Int(size.width))), h = min(4096, max(48, Int(size.height)))
            guard w * h <= maximumPixels else { return }
            self.transmit(["type": "window.configure", "window_id": self.id, "width": w, "height": h])
        }
    }
    func closeLocally() { resizeTimer?.invalidate(); allowClose = true; surface.transmit = nil; window.close() }
}

final class AppDelegate: NSObject, NSApplicationDelegate {
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
        alert("LAN RemoteApp P1 开发预览\n原生 AppKit 窗口 + PNG。此版本尚未完成真实 Mac/Ubuntu 联调；中文输入、Retina 远端重绘、剪贴板与 Metal 暂未实现。")
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
        // The bridge owns one attached session. Only session.resume carries session_id.
        transport.send(message)
    }
    func receive(_ incoming: Incoming) {
        let object = incoming.object
        guard let type = object["type"] as? String else { return }
        if type == "hello" {
            guard !ready else { return }; ready = true
            if let app = options.app { send(["type": "app.launch", "app_id": app, "request_id": UUID().uuidString, "backend": "native"]) }
            else if let session = options.session { send(["type": "session.resume", "session_id": session]) }
            return
        }
        if type == "error" {
            // Display server error code, not an arbitrary message potentially containing app data.
            let code = object["code"] as? String ?? "unknown"
            alert("远端返回错误：" + String(code.filter { $0.isASCII && ($0.isLetter || $0.isNumber || $0 == "_") }.prefix(80))); return
        }
        if type == "session.started" || type == "session.snapshot" {
            guard let id = object["session_id"] as? String, id.count <= 128 else { return }
            if let session = session, session != id { return }
            session = id; statusItem?.button?.title = "RemoteApp · 已连接"
        }
        guard let currentSession = session, object["session_id"] as? String == currentSession else { return }
        guard let seq = unsignedInteger(object["seq"]) else { transport.disconnect(); return }
        guard seq > lastSequence else { return }; lastSequence = seq
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
        default: break
        }
    }
    func updateWindow(_ object: [String: Any], create: Bool) {
        guard let number = unsignedInteger(object["window_id"]), number <= UInt64(UInt32.max),
              let w = object["width"] as? Int, let h = object["height"] as? Int,
              w > 0, h > 0, w <= 4096, h <= 4096, w * h <= maximumPixels,
              let rev = unsignedInteger(object["geometry_revision"]) else { return }
        let id = UInt32(number)
        if removed.contains(id) { return } // XID reuse in one connection is deliberately not supported.
        let otherPixels = windows.filter { $0.key != id }.values.reduce(0) { $0 + $1.surface.remoteWidth * $1.surface.remoteHeight }
        guard otherPixels + w * h <= 16_000_000 else { transport.disconnect(); return }
        if windows[id] == nil {
            guard create, windows.count < 64 else { return }
            let host = HostedWindow(id: id, width: w, height: h, revision: rev,
                popup: object["role"] as? String == "popup") { [weak self] in self?.send($0) }
            windows[id] = host; host.window.makeKeyAndOrderFront(nil)
        }
        guard let host = windows[id], rev >= host.revision else { return }
        host.revision = rev
        host.window.title = String((object["title"] as? String ?? "Linux 应用").prefix(512))
        host.surface.remoteWidth = w; host.surface.remoteHeight = h
        host.applyingRemote = true
        if Int(host.surface.bounds.width) != w || Int(host.surface.bounds.height) != h { host.window.setContentSize(NSSize(width: CGFloat(w), height: CGFloat(h))) }
        host.applyingRemote = false
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
