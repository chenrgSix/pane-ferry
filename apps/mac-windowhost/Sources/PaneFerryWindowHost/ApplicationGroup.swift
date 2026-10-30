import AppKit

/// One AppKit application owns independent session controllers. Window IDs and
/// transport state are deliberately scoped to a controller, never to this group.
final class ApplicationGroup: NSObject, NSApplicationDelegate {
    let options: Options
    let icon: NSImage?
    let broker: ApplicationBroker
    private(set) var sessions: [String: AppDelegate] = [:]
    private var targets: [String: String] = [:]
    private var detaching = Set<String>()
    private var statusItem: NSStatusItem?
    private var terminationSignal: DispatchSourceSignal?
    private var quitting = false
    var makeSession: (Options, NSImage?) -> AppDelegate = { AppDelegate(options: $0, applicationIcon: $1) }
    var startSession: (AppDelegate) -> Void = { $0.startSession(managesApplication: false) }
    var terminateApplication: () -> Void = { NSApp.terminate(nil) }

    init(options: Options, icon: NSImage?) throws {
        guard let group = options.applicationGroup else { throw HostError.message("缺少应用接收入口") }
        self.options = options; self.icon = icon
        broker = try ApplicationBroker(group: group)
        super.init()
        broker.handle = { [weak self] in self?.request($0, client: $1) }
    }
    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.regular)
        if let icon = icon { NSApp.applicationIconImage = icon }
        signal(SIGTERM, SIG_IGN)
        let signalSource = DispatchSource.makeSignalSource(signal: SIGTERM, queue: .main)
        signalSource.setEventHandler { [weak self] in self?.terminateApplication() }
        signalSource.resume(); terminationSignal = signalSource
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        let menu = NSMenu()
        menu.addItem(withTitle: "前置全部窗口", action: #selector(NSApplication.arrangeInFront(_:)), keyEquivalent: "")
        menu.addItem(.separator())
        menu.addItem(withTitle: "断开全部会话并退出（保留远端应用）", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        statusItem?.menu = menu
        if let session = options.session { _ = attach(session, target: options.sourceTarget, fps: options.requestedVideoFPS, pictureMode: options.pictureMode, background: options.backgroundStart) }
    }
    @discardableResult
    func attach(_ id: String, target: String?, fps: Int, pictureMode: PictureMode = .adapt, background: Bool) -> Bool {
        guard !quitting, !detaching.contains(id) else { return false }
        if let controller = sessions[id] {
            if !background { select(controller); controller.windows.values.first { !$0.popup }?.window.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true) }
            return true
        }
        guard sessions.count < 32 else { return false }
        // Group options always use --session, so each connection resumes only
        // its own session and cannot relaunch another application's contents.
        let sessionOptions = Options(app: nil, session: id, requestedVideoFPS: fps, pictureMode: pictureMode, backgroundStart: background,
                                 applicationGroup: options.applicationGroup, sourceTarget: target, native: options.native)
        let controller = makeSession(sessionOptions, icon)
        sessions[id] = controller; targets[id] = target
        controller.terminateHost = { [weak self] in self?.remove(id) }
        controller.disconnectSession = { [weak self] in self?.detach(id) }
        controller.onFocus = { [weak self, weak controller] in if let controller = controller { self?.select(controller) } }
        startSession(controller)
        if sessions.count == 1 || !background { select(controller) }
        updateStatus(); return true
    }
    private func select(_ controller: AppDelegate) { controller.installMenu() }
    private func updateStatus() { statusItem?.button?.title = "PaneFerry · \(sessions.count) 个会话" }
    func detach(_ id: String) {
        guard let controller = sessions[id], detaching.insert(id).inserted else { return }
        controller.detachSession { [weak self] in self?.remove(id) }
    }
    private func remove(_ id: String) {
        guard let controller = sessions.removeValue(forKey: id) else { return }
        controller.applicationWillTerminate(Notification(name: NSApplication.willTerminateNotification))
        for window in controller.windows.values { window.closeLocally() }
        controller.windows.removeAll(); controller.parents.removeAll()
        targets.removeValue(forKey: id); detaching.remove(id); broker.closed(id)
        if let next = sessions.values.first(where: { $0.activeHost() != nil }) ?? sessions.values.first { select(next) }
        updateStatus()
        if sessions.isEmpty && !quitting { terminateApplication() }
    }
    private func request(_ request: [String: Any], client: BrokerClient) {
        guard unsignedInteger(request["version"]) == 1, request["group"] as? String == options.applicationGroup,
              !quitting else { client.shutdown(); return }
        if request["op"] as? String == "list" {
            client.reply(["version": 1, "pid": getpid(), "sessions": sessions.keys.sorted().map {
                ["session": $0, "target": targets[$0] ?? ""]
            }]); client.shutdown(); return
        }
        guard let id = request["session"] as? String, id.count <= 128,
              id.range(of: "^[A-Za-z0-9][A-Za-z0-9_.-]*$", options: .regularExpression) != nil else { client.shutdown(); return }
        if request["op"] as? String == "attach", let fps = unsignedInteger(request["fps"]), [30, 60].contains(fps),
           let pictureMode = PictureMode(rawValue: request["picture_mode"] as? String ?? "adapt"),
           let background = request["background"] as? Bool,
           attach(id, target: request["target"] as? String, fps: Int(fps), pictureMode: pictureMode, background: background) {
            broker.watch(id, client: client)
            client.reply(["version": 1, "pid": getpid(), "session": id]); return
        }
        if request["op"] as? String == "detach" {
            if sessions[id] == nil { client.reply(["version": 1, "session": id, "closed": true]); client.shutdown() }
            else { broker.watch(id, client: client); detach(id) }
            return
        }
        client.reply(["version": 1, "error": "应用会话正在关闭或数量已达上限，请稍后重试。"]); client.shutdown()
    }
    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        if !flag { for controller in sessions.values { for window in controller.windows.values where !window.popup && window.remoteVisible { window.window.makeKeyAndOrderFront(nil) } } }
        return true
    }
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        guard !quitting else { return .terminateLater }
        quitting = true
        let controllers = Array(sessions.values)
        guard !controllers.isEmpty else { return .terminateNow }
        var remaining = controllers.count
        for controller in controllers {
            controller.detachSession {
                remaining -= 1
                if remaining == 0 { sender.reply(toApplicationShouldTerminate: true) }
            }
        }
        return .terminateLater
    }
    func applicationWillTerminate(_ notification: Notification) {
        for controller in sessions.values { controller.applicationWillTerminate(notification) }
        broker.stop()
        if let item = statusItem { NSStatusBar.system.removeStatusItem(item) }; statusItem = nil
    }
}
