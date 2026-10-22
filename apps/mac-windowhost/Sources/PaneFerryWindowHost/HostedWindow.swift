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
    var scaleMode = "unknown"
    var resolutionNotice: String? { didSet { updateSubtitle() } }
    var inputNotice: String? { didSet { updateSubtitle() } }
    var fixedPixels: (width: Int, height: Int)?
    private(set) var requestedPixels: (width: Int, height: Int)?
    private var resolutionTimer: Timer?
    private var resolutionDelayed = false
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
        let frame = surface.image
        var parts = [frame.map { "接收 \($0.width) × \($0.height) 像素" } ?? "等待画面"]
        if let target = requestedPixels {
            let matched = frame?.width == target.width && frame?.height == target.height
            if !matched {
                parts.append("目标 \(target.width) × \(target.height)")
                parts.append(resolutionDelayed ? "远端尚未匹配 · 等比显示" : "等待重绘 · 等比显示")
            } else {
                resolutionTimer?.invalidate(); resolutionTimer = nil; resolutionDelayed = false
            }
            let physical = surface.bounds.size
            if fixedPixels == nil && (target.width < Int((physical.width * window.backingScaleFactor).rounded()) ||
                                      target.height < Int((physical.height * window.backingScaleFactor).rounded())) {
                parts.append("已达像素预算")
            }
        }
        switch scaleMode {
        case "source": parts.append("沿用应用缩放")
        case "requested": parts.append("启动缩放请求 \(rendering.scale * 100)% · 未确认")
        case "verified": parts.append("应用缩放 \(rendering.scale * 100)%")
        default: parts.append("应用缩放未报告")
        }
        if let mediaMode = mediaMode { parts.append(mediaMode) }
        if let frame = frame {
            let rect = surface.imageRect
            let enlargement = max(rect.width * window.backingScaleFactor / CGFloat(frame.width),
                                  rect.height * window.backingScaleFactor / CGFloat(frame.height))
            if enlargement > 1.05 { parts.append("画面放大 \(Int((enlargement * 100).rounded()))%") }
        }
        if let resolutionNotice = resolutionNotice { parts.append(resolutionNotice) }
        if let inputNotice = inputNotice { parts.append(inputNotice) }
        window.subtitle = parts.joined(separator: " · ")
    }
    func present(_ image: CGImage, mediaMode: String) {
        surface.image = image; self.mediaMode = mediaMode
        updateSubtitle()
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
        requestedPixels = pixels
        resolutionNotice = nil
        resolutionTimer?.invalidate(); resolutionDelayed = false
        updateSubtitle()
        if pixels.width == surface.remoteWidth && pixels.height == surface.remoteHeight { return }
        transmit(["type": "window.configure", "window_id": id, "width": pixels.width, "height": pixels.height])
        let timer = Timer(timeInterval: 1.5, repeats: false) { [weak self] _ in
            self?.resolutionDelayed = true; self?.updateSubtitle()
        }
        resolutionTimer = timer; RunLoop.main.add(timer, forMode: .common)
    }
    func useAutomaticResolution() {
        fixedPixels = nil
        windowDidResize(Notification(name: NSWindow.didResizeNotification))
    }
    func showActualPixels() {
        guard let image = surface.image else { return }
        fixedPixels = (image.width, image.height)
        window.setContentSize(NSSize(width: CGFloat(image.width) / window.backingScaleFactor,
                                     height: CGFloat(image.height) / window.backingScaleFactor))
        windowDidResize(Notification(name: NSWindow.didResizeNotification))
    }
    func fitToScreen() {
        guard let screen = window.screen ?? NSScreen.main else { return }
        fixedPixels = nil
        window.setFrame(screen.visibleFrame, display: true, animate: false)
        windowDidResize(Notification(name: NSWindow.didResizeNotification))
    }
    func closeLocally() { resizeTimer?.invalidate(); resolutionTimer?.invalidate(); allowClose = true; surface.transmit = nil; window.close() }
}
