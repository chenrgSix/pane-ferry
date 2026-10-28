import AppKit

// Exercise the real AppKit nested termination loop. A mock terminateHost would
// miss the deadlock when session completion arrives on the serial main queue.
private final class TerminationDelegate: NSObject, NSApplicationDelegate {
    let host: AppDelegate
    init(_ host: AppDelegate) { self.host = host }
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        host.applicationShouldTerminate(sender)
    }
    func applicationWillTerminate(_ notification: Notification) {
        host.applicationWillTerminate(notification)
        print("PASS: bounded AppKit termination from the main dispatch queue")
    }
}
@main struct TerminationSmoke {
    static func main() {
        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)
        let host = AppDelegate(options: Options(app: "termination-fixture", session: nil,
            native: NativeRoute(peer: "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
                state: "/tmp/unused-termination-fixture", helper: "/usr/bin/false")))
        host.wireSink = { _ in }
        host.observeTerminationSignal()
        let delegate = TerminationDelegate(host)
        app.delegate = delegate
        DispatchQueue.global().asyncAfter(deadline: .now() + 3) {
            FileHandle.standardError.write(Data("FAIL: AppKit termination blocked\n".utf8))
            exit(2)
        }
        DispatchQueue.main.async {
            if CommandLine.arguments.last == "session" { host.finishSession() }
            else if CommandLine.arguments.last == "signal" { kill(getpid(), SIGTERM) }
            else { app.terminate(nil) }
        }
        withExtendedLifetime(delegate) { app.run() }
    }
}
