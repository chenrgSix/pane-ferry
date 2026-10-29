import AppKit

// Exercise the real AppKit nested termination loop. A mock terminateHost would
// miss the deadlock when session completion arrives on the serial main queue.
private final class TerminationDelegate: NSObject, NSApplicationDelegate {
    let host: AppDelegate
    let group: ApplicationGroup?
    init(_ host: AppDelegate, group: ApplicationGroup? = nil) { self.host = host; self.group = group }
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        group?.applicationShouldTerminate(sender) ?? host.applicationShouldTerminate(sender)
    }
    func applicationWillTerminate(_ notification: Notification) {
        if let group = group {
            group.applicationWillTerminate(notification)
            precondition(group.sessions.values.allSatisfy { $0.stopping && $0.notice.current == nil })
            unlink(ApplicationBroker.path(group: group.options.applicationGroup!))
        } else { host.applicationWillTerminate(notification) }
        precondition(host.notice.current == nil, "termination must remove the session's notice")
        print("PASS: bounded AppKit termination from the main dispatch queue")
    }
}
@main struct TerminationSmoke {
    static func main() throws {
        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)
        let mode = CommandLine.arguments.last ?? "quit"
        var host = AppDelegate(options: Options(app: "termination-fixture", session: nil,
            native: NativeRoute(peer: "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
                state: "/tmp/unused-termination-fixture", helper: "/usr/bin/false")))
        var group: ApplicationGroup?
        if mode.hasPrefix("group-") {
            let groupID = UUID().uuidString.replacingOccurrences(of: "-", with: "").lowercased() +
                UUID().uuidString.replacingOccurrences(of: "-", with: "").lowercased()
            try FileManager.default.createDirectory(atPath: "/tmp/pf-app-\(geteuid())", withIntermediateDirectories: true,
                                                    attributes: [.posixPermissions: 0o700])
            let options = Options(app: nil, session: "one", backgroundStart: true,
                                  applicationGroup: groupID, native: host.options.native)
            let owner = try ApplicationGroup(options: options, icon: nil)
            owner.makeSession = { options, icon in
                let controller = AppDelegate(options: options, applicationIcon: icon)
                controller.wireSink = { _ in }
                return controller
            }
            owner.startSession = { _ in }
            precondition(owner.attach("one", target: nil, fps: 30, background: true))
            precondition(owner.attach("two", target: nil, fps: 30, background: true))
            host = owner.sessions["one"]!; group = owner
        }
        host.wireSink = { _ in }
        if let group = group {
            group.applicationDidFinishLaunching(Notification(name: NSApplication.didFinishLaunchingNotification))
        } else { host.observeTerminationSignal() }
        let delegate = TerminationDelegate(host, group: group)
        app.delegate = delegate
        DispatchQueue.global().asyncAfter(deadline: .now() + 3) {
            FileHandle.standardError.write(Data("FAIL: AppKit termination blocked\n".utf8))
            exit(2)
        }
        DispatchQueue.main.async {
            if mode.contains("notice-") {
                let controllers = group.map { Array($0.sessions.values) } ?? [host]
                for controller in controllers {
                    for _ in 0..<100 {
                        controller.receive(Incoming(object: ["type": "error", "code": "capture_failed"], image: nil))
                    }
                    precondition(controller.notice.current?.window.isVisible == true)
                }
                precondition(app.modalWindow == nil)
            }
            if mode == "notice-acknowledge" { host.notice.current!.buttons[0].performClick(nil) }
            else if mode == "notice-close" { host.notice.current!.window.performClose(nil) }
            else if mode == "notice-disconnect" { host.notice.current!.buttons[1].performClick(nil) }
            else if mode == "session" || mode == "notice-session" { host.finishSession() }
            else if mode.hasSuffix("signal") { kill(getpid(), SIGTERM) }
            else { app.terminate(nil) }
        }
        withExtendedLifetime(delegate) { app.run() }
    }
}
