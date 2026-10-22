import AppKit
import Darwin
/// The production Node with only its state directory isolated for live tests.
@main struct MacNodeHarness {
    static func main() {
        guard CommandLine.arguments.count==2 else { exit(2) }
        signal(SIGPIPE,SIG_IGN)
        let app=NSApplication.shared;app.setActivationPolicy(.accessory)
        MainActor.assumeIsolated { do {
            let node=try Node(URL(fileURLWithPath:CommandLine.arguments[1]));app.delegate=node
            withExtendedLifetime(node) { app.run() }
        } catch { exit(1) } }
    }
}
