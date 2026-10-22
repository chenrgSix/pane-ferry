import AppKit
import ScreenCaptureKit
import Darwin
// Readiness probes intentionally connect and close without a hello. Closed
// peers must only end their connection, never terminate the graphical service.
signal(SIGPIPE,SIG_IGN)


let directory=FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/io.lanremoteapp.launcher/Node")
let app=NSApplication.shared
app.setActivationPolicy(.accessory)
MainActor.assumeIsolated { do {
    let node=try Node(directory)
    app.delegate=node
    withExtendedLifetime(node) { app.run() }
} catch { fputs("窗渡 PaneFerry: service configuration unavailable\n",stderr);exit(1) } }
