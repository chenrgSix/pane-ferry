import AppKit

final class HostedWindow: NSObject, NSWindowDelegate {
    let id: UInt32
    let window: NSWindow
    let surface = SurfaceView(frame: .zero)
    let popup: Bool
    var revision: UInt64
    var applyingRemote = false
    var remoteVisible = false
    var allowClose = false
    var relatedWindow = false
    var resizeTimer: Timer?
    var rendering: RenderGeometry
    var mediaMode: String?
    var inputNotice: String? { didSet { updateSubtitle() } }
    var fixedPixels: (width: Int, height: Int)? {
        didSet { surface.preservesAspectRatio = fixedPixels != nil }
    }
    var transmit: ([String: Any]) -> Void
    private let disconnect: () -> Void
    private var disconnectRequested = false

    init(id: UInt32, width: Int, height: Int, revision: UInt64, scale: Int, popup: Bool, disconnect: @escaping () -> Void, transmit: @escaping ([String: Any]) -> Void) {
        self.id = id; self.revision = revision; self.popup = popup; self.transmit = transmit
        self.disconnect = disconnect
        self.rendering = RenderGeometry(scale: scale)!
        let size = rendering.points(width: width, height: height)
        let style: NSWindow.StyleMask = popup ? [.borderless] : [.titled, .closable, .miniaturizable, .resizable]
        self.window = NSWindow(contentRect: NSRect(origin: .zero, size: size), styleMask: style, backing: .buffered, defer: false)
        super.init()
        window.delegate = self; window.isReleasedWhenClosed = false
        window.contentMinSize = NSSize(width: 64, height: 48)
        window.acceptsMouseMovedEvents = true
        surface.remoteID = id; surface.remoteWidth = width; surface.remoteHeight = height; surface.transmit = transmit
        window.contentView = surface; window.center()
        window.makeFirstResponder(surface)
    }
    func applyRemoteGeometry(width: Int, height: Int, rendering: RenderGeometry) {
        self.rendering = rendering
        surface.remoteWidth = width; surface.remoteHeight = height
        // Replies can describe an earlier resize. The controller owns the
        // viewport; only remote-sized popup windows follow incoming geometry.
        if popup {
            applyingRemote = true
            window.setContentSize(rendering.points(width: width, height: height))
            applyingRemote = false
        }
        surface.needsDisplay = true
        updateSubtitle()
    }
    func updateSubtitle() {
        window.subtitle = inputNotice ?? ("\(surface.remoteWidth) × \(surface.remoteHeight) 像素 · 界面 \(rendering.scale * 100)%" + (mediaMode.map { " · " + $0 } ?? ""))
    }
    func windowShouldClose(_ sender: NSWindow) -> Bool {
        if allowClose { return true }
        if relatedWindow {
            resizeTimer?.invalidate(); resizeTimer = nil
            surface.releaseInput()
            transmit(["type":"window.close.request", "window_id":id])
            return false
        }
        if !disconnectRequested {
            disconnectRequested = true
            resizeTimer?.invalidate(); resizeTimer = nil
            surface.releaseInput()
            disconnect()
        }
        return false
    }
    func windowDidBecomeKey(_ notification: Notification) {
        guard remoteVisible else { return }
        transmit(["type": "window.focus", "window_id": id])
    }
    func windowDidResignKey(_ notification: Notification) { surface.releaseInput() }
    func windowDidResize(_ notification: Notification) {
        guard !applyingRemote, remoteVisible, !popup, resizeTimer == nil else { return }
        // Common modes keep this running during AppKit's live resize loop.
        // Coalesce to the latest bounds without postponing every update.
        let timer = Timer(timeInterval: 0.10, repeats: false) { [weak self] _ in self?.sendResize() }
        resizeTimer = timer
        RunLoop.main.add(timer, forMode: .common)
    }
    func windowDidEndLiveResize(_ notification: Notification) { sendResize() }
    func windowDidChangeBackingProperties(_ notification: Notification) { windowDidResize(notification) }
    private func sendResize() {
        resizeTimer?.invalidate(); resizeTimer = nil
        guard !applyingRemote, remoteVisible, !popup else { return }
        guard let pixels = fixedPixels ?? rendering.pixels(for: surface.bounds.size, backingScale: window.backingScaleFactor) else { return }
        if pixels.width == surface.remoteWidth && pixels.height == surface.remoteHeight { return }
        transmit(["type": "window.configure", "window_id": id, "width": pixels.width, "height": pixels.height])
    }
    func useAutomaticResolution() {
        fixedPixels = nil
        windowDidResize(Notification(name: NSWindow.didResizeNotification))
    }
    func fitToScreen() {
        guard let screen = window.screen ?? NSScreen.main else { return }
        fixedPixels = nil
        window.setFrame(screen.visibleFrame, display: true, animate: false)
        windowDidResize(Notification(name: NSWindow.didResizeNotification))
    }
    func closeLocally() { resizeTimer?.invalidate(); allowClose = true; surface.transmit = nil; window.close() }
}
