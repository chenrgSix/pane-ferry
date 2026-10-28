import AppKit

private final class SurfaceWindow: NSWindow {
    var overlay = false
    override var canBecomeKey: Bool { !overlay && super.canBecomeKey }
    override var canBecomeMain: Bool { !overlay && super.canBecomeMain }
}

final class HostedWindow: NSObject, NSWindowDelegate {
    let id: UInt32
    let window: NSWindow
    let surface = SurfaceView(frame: .zero)
    private(set) var popup: Bool
    var presentation: WindowPresentation?
    var onLayoutChange: (() -> Void)?
    private var lastPlacement: NSRect?
    private(set) var overflowViewport: NSScrollView?
    var desktopMode = false
    var sessionMode = false
    var fixedCanvas: Bool { desktopMode || sessionMode }
    var revision: UInt64
    var applyingRemote = false
    var remoteVisible = false
    private var receivedVisibility = false
    let hiddenSourceView = NSVisualEffectView(frame: .zero)
    private(set) var restoreSourceButton: NSButton!
    var allowClose = false
    private var closeRequested = false
    var relatedWindow = false
    var activateOnShow = true
    var resizeTimer: Timer?
    var rendering: RenderGeometry
    var mediaMode: String?
    var scaleMode = "unknown"
    var resolutionNotice: String? { didSet { updateSubtitle() } }
    var inputNotice: String? { didSet { updateSubtitle() } }
    var onScreenChange: (() -> Void)?
    var fixedPixels: (width: Int, height: Int)?
    private(set) var requestedPixels: (width: Int, height: Int)?
    private var resolutionTimer: Timer?
    private var reconciliationTimer: Timer?
    private var reconciliationAttempts = 2
    private var resolutionDelayed = false
    private var recentFrames: [TimeInterval] = []
    private var hasReceivedFrame = false
    var transmit: ([String: Any]) -> Void
    private let disconnect: () -> Void

    init(id: UInt32, width: Int, height: Int, revision: UInt64, scale: Int, popup: Bool, disconnect: @escaping () -> Void, transmit: @escaping ([String: Any]) -> Void) {
        self.id = id; self.revision = revision; self.popup = popup; self.transmit = transmit
        self.disconnect = disconnect
        self.rendering = RenderGeometry(scale: scale)!
        let size = rendering.points(width: width, height: height)
        let style: NSWindow.StyleMask = popup ? [.borderless] : [.titled, .closable, .miniaturizable, .resizable]
        let nativeWindow = SurfaceWindow(contentRect: NSRect(origin: .zero, size: size), styleMask: style, backing: .buffered, defer: false)
        nativeWindow.overlay = popup
        self.window = nativeWindow
        super.init()
        window.delegate = self; window.isReleasedWhenClosed = false
        window.contentMinSize = popup ? NSSize(width: 1, height: 1) : NSSize(width: 64, height: 48)
        window.acceptsMouseMovedEvents = true
        surface.remoteID = id; surface.remoteWidth = width; surface.remoteHeight = height; surface.transmit = transmit
        surface.overlay = popup
        window.contentView = surface; window.center()
        window.makeFirstResponder(surface)
        hiddenSourceView.frame = surface.bounds
        hiddenSourceView.autoresizingMask = [.width, .height]
        hiddenSourceView.material = .windowBackground
        hiddenSourceView.blendingMode = .withinWindow
        hiddenSourceView.isHidden = true
        let notice = NSTextField(wrappingLabelWithString: "源端窗口已最小化或隐藏")
        notice.alignment = .center
        restoreSourceButton = NSButton(title: "恢复源端窗口", target: self, action: #selector(restoreSourceWindow))
        let stack = NSStackView(views: [notice, restoreSourceButton])
        stack.orientation = .vertical; stack.spacing = 16
        stack.translatesAutoresizingMaskIntoConstraints = false
        hiddenSourceView.addSubview(stack); surface.addSubview(hiddenSourceView)
        // The restore notice must not impose its intrinsic width on a small
        // source popup, even while this view is hidden.
        let noticeWidth = stack.widthAnchor.constraint(lessThanOrEqualTo: hiddenSourceView.widthAnchor, constant: -24)
        noticeWidth.priority = .defaultLow
        NSLayoutConstraint.activate([
            stack.centerXAnchor.constraint(equalTo: hiddenSourceView.centerXAnchor),
            stack.centerYAnchor.constraint(equalTo: hiddenSourceView.centerYAnchor),
            noticeWidth
        ])
    }
    func applyPresentation(_ value: WindowPresentation, related: Bool, role: String) {
        let overlay = role == "popup" || (related && value.overlay)
        if popup != overlay {
            surface.releaseInput()
            popup = overlay
            (window as? SurfaceWindow)?.overlay = overlay
            window.styleMask = overlay ? [.borderless] : [.titled, .closable, .miniaturizable, .resizable]
            window.contentMinSize = overlay ? NSSize(width: 1, height: 1) : NSSize(width: 64, height: 48)
        }
        relatedWindow = related
        surface.overlay = overlay
        presentation = value
        window.ignoresMouseEvents = overlay && value.passive
        window.hasShadow = !overlay // Don't add a second native shadow to source pixels.
        if overlay { resizeTimer?.invalidate(); resizeTimer = nil }
        if overlay || related { cancelResolutionReconciliation() }
        else { removeOverflowViewport() }
    }
    func sceneDisplacement(relativeTo parent: HostedWindow) -> CGPoint {
        guard let placement = presentation, let anchor = parent.presentation else { return .zero }
        let rect = placement.relativeRect(to: anchor, in: parent.surface.imageRect)
        let source = parent.window.convertToScreen(parent.surface.convert(rect, to: nil))
        let displayed = window.convertToScreen(surface.convert(surface.imageRect, to: nil))
        return CGPoint(x: displayed.minX - source.minX, y: displayed.maxY - source.maxY)
    }
    func place(relativeTo parent: HostedWindow, available: Bool, sceneOffset: CGPoint = .zero) {
        guard let placement = presentation, let anchor = parent.presentation, available,
              remoteVisible, parent.remoteVisible, parent.window.isVisible, !parent.window.isMiniaturized,
              let image = parent.surface.displayFrame, image.width == parent.surface.remoteWidth,
              image.height == parent.surface.remoteHeight,
              surface.displayFrame?.width == surface.remoteWidth, surface.displayFrame?.height == surface.remoteHeight else {
            window.orderOut(nil); return
        }
        let rect = placement.relativeRect(to: anchor, in: parent.surface.imageRect)
        let content = parent.window.convertToScreen(parent.surface.convert(rect, to: nil))
            .offsetBy(dx: sceneOffset.x, dy: sceneOffset.y)
        let viewport = popup ? (parent.window.screen ?? NSScreen.main).map {
            OverlayViewport(content: content, available: $0.visibleFrame)
        } : nil
        let frame = window.frameRect(forContentRect: viewport?.frame ?? content)
        applyingRemote = true
        if let viewport = viewport, viewport.scrolls {
            installOverflowViewport()
        } else { removeOverflowViewport() }
        // AppKit can move child windows with their parent, independently of our
        // last source placement. Compare the actual frame as well.
        if frame != lastPlacement || window.frame != frame { window.setFrame(frame, display: true); lastPlacement = frame }
        if let scroll = overflowViewport, let viewport = viewport {
            let origin = scroll.contentView.bounds.origin
            surface.setFrameSize(viewport.documentSize)
            scroll.hasVerticalScroller = viewport.documentSize.height > viewport.frame.height
            scroll.hasHorizontalScroller = viewport.documentSize.width > viewport.frame.width
            scroll.tile()
            scroll.contentView.scroll(to: CGPoint(
                x: min(max(0, origin.x), max(0, surface.frame.width - scroll.contentSize.width)),
                y: min(max(0, origin.y), max(0, surface.frame.height - scroll.contentSize.height))))
            scroll.reflectScrolledClipView(scroll.contentView)
        }
        applyingRemote = false
        // Overlays keep the keyboard responder in their owner. Ordinary
        // dialogs retain their existing ability to receive typing on opening.
        if !window.isVisible {
            if popup { window.orderFront(nil) }
            else { window.makeKeyAndOrderFront(nil) }
        }
    }
    private func installOverflowViewport() {
        guard overflowViewport == nil else { return }
        let scroll = NSScrollView(frame: surface.frame)
        scroll.borderType = .noBorder
        scroll.drawsBackground = false
        scroll.scrollerStyle = .overlay
        scroll.autohidesScrollers = false
        scroll.horizontalScrollElasticity = .none
        scroll.verticalScrollElasticity = .none
        surface.removeFromSuperview()
        surface.autoresizingMask = []
        scroll.documentView = surface
        window.contentView = scroll
        overflowViewport = scroll
        surface.scrollOverflowLocally = true
        scroll.contentView.postsBoundsChangedNotifications = true
        NotificationCenter.default.addObserver(self, selector: #selector(viewportScrolled),
            name: NSView.boundsDidChangeNotification, object: scroll.contentView)
        window.makeFirstResponder(surface)
    }
    private func removeOverflowViewport() {
        guard let scroll = overflowViewport else { return }
        NotificationCenter.default.removeObserver(self, name: NSView.boundsDidChangeNotification, object: scroll.contentView)
        scroll.documentView = nil
        surface.scrollOverflowLocally = false
        window.contentView = surface
        overflowViewport = nil
        window.makeFirstResponder(surface)
    }
    @objc private func viewportScrolled(_ notification: Notification) {
        // Descendant overlays use the document-to-screen transform, including
        // this viewport's offset, rather than an unscrolled source origin.
        if !applyingRemote { onLayoutChange?() }
    }
    func applyRemoteVisibility(_ visible: Bool) {
        let changed = !receivedVisibility || visible != remoteVisible
        receivedVisibility = true
        if !visible {
            surface.releaseInput(); surface.unmarkText()
            resizeTimer?.invalidate(); resizeTimer = nil
            cancelResolutionReconciliation()
            if let scroll = overflowViewport {
                applyingRemote = true
                scroll.contentView.scroll(to: .zero)
                scroll.reflectScrolledClipView(scroll.contentView)
                applyingRemote = false
            }
        }
        remoteVisible = visible
        surface.transmit = visible ? transmit : nil
        hiddenSourceView.isHidden = visible || popup || relatedWindow || desktopMode
        if !visible { surface.requestControllerPaste = nil }
        if changed && !relatedWindow && !popup {
            if visible || !hiddenSourceView.isHidden {
                // A remote visibility change must not undo a local minimization.
                if !window.isMiniaturized {
                    if activateOnShow { window.makeKeyAndOrderFront(nil) }
                    else { window.orderFront(nil) }
                }
            } else { window.orderOut(nil) }
        }
        if !visible && (relatedWindow || popup) { window.orderOut(nil) }
        if visible && changed { reconcileResolution() }
        updateSubtitle()
    }
    @objc func restoreSourceWindow() {
        guard !remoteVisible, !popup, !relatedWindow, !desktopMode else { return }
        // Wait for source metadata before enabling input or showing old pixels.
        transmit(["type": "window.focus", "window_id": id])
    }
    func applyRemoteGeometry(width: Int, height: Int, rendering: RenderGeometry) {
        let scaleChanged = self.rendering.scale != rendering.scale
        self.rendering = rendering
        surface.remoteWidth = width; surface.remoteHeight = height
        // Replies can describe an earlier resize. The controller owns the
        // viewport; only remote-sized popup windows follow incoming geometry.
        if popup && !relatedWindow {
            applyingRemote = true
            window.setContentSize(rendering.points(width: width, height: height))
            applyingRemote = false
        }
        surface.needsDisplay = true
        updateSubtitle()
        // A source can move to another monitor. Keep the local logical size
        // and convert it using the newly reported source density only.
        if scaleChanged && fixedPixels == nil && !relatedWindow { windowDidResize(Notification(name: NSWindow.didResizeNotification)) }
        reconcileResolution()
    }
    private func cancelResolutionReconciliation() {
        reconciliationTimer?.invalidate(); reconciliationTimer = nil
    }
    private func matchesRequestedSize(width: Int, height: Int) -> Bool {
        guard let target = requestedPixels else { return false }
        // Integer compositor coordinates at 2x can round an odd pixel request
        // by one pixel. Keep displaying the real dimensions, without retrying
        // a physically unrepresentable half logical pixel.
        return abs(target.width - width) <= 1 && abs(target.height - height) <= 1
    }
    private func reconcileResolution() {
        guard requestedPixels != nil else { return }
        if matchesRequestedSize(width: surface.remoteWidth, height: surface.remoteHeight) {
            cancelResolutionReconciliation(); reconciliationAttempts = 2
            return
        }
        // Source maximization/restoration can change geometry without an AppKit
        // resize. Reassert the controller's viewport after transient replies
        // settle, but stop if a WM minimum/increment prevents an exact match.
        guard !fixedCanvas, !relatedWindow, !popup, remoteVisible, !closeRequested, !allowClose,
              resolutionNotice == nil, reconciliationAttempts > 0,
              resizeTimer == nil, reconciliationTimer == nil else { return }
        let timer = Timer(timeInterval: 0.35, repeats: false) { [weak self] _ in
            guard let self = self else { return }
            self.reconciliationTimer = nil
            self.sendResize(reconciling: true)
        }
        reconciliationTimer = timer; RunLoop.main.add(timer, forMode: .common)
    }
    func updateSubtitle() {
        let now = ProcessInfo.processInfo.systemUptime
        recentFrames.removeAll { now - $0 > 2 }
        let frame = surface.displayFrame
        var parts = [frame.map { "接收 \($0.width) × \($0.height) 像素" } ?? "等待画面"]
        if fixedPixels == nil {
            parts.append("本机 \(Int(surface.bounds.width.rounded())) × \(Int(surface.bounds.height.rounded())) 点 · 等比显示")
        }
        if desktopMode { parts.append("远端显示器原始画面") }
        if sessionMode { parts.append("应用会话画面") }
        if let target = requestedPixels {
            let matched = frame.map { matchesRequestedSize(width: $0.width, height: $0.height) } ?? false
            if !matched {
                parts.append("目标 \(target.width) × \(target.height)")
                parts.append(resolutionDelayed ? "远端尚未匹配 · 等比显示" : "等待重绘 · 等比显示")
            } else {
                resolutionTimer?.invalidate(); resolutionTimer = nil; resolutionDelayed = false
            }
            let logical = surface.bounds.size
            if fixedPixels == nil && (target.width < Int((logical.width * CGFloat(rendering.scale)).rounded()) ||
                                      target.height < Int((logical.height * CGFloat(rendering.scale)).rounded())) {
                parts.append("已达像素预算")
            }
        }
        switch fixedCanvas ? "desktop" : scaleMode {
        case "desktop": break
        case "source": parts.append("沿用应用缩放")
        case "requested": parts.append("启动缩放请求 \(rendering.scale * 100)% · 未确认")
        case "verified": parts.append("应用缩放 \(rendering.scale * 100)%")
        default: parts.append("应用缩放未报告")
        }
        if let mediaMode = mediaMode { parts.append(mediaMode) }
        if hasReceivedFrame {
            if recentFrames.isEmpty {
                parts.append("近 2 秒未收到新画面")
            } else if recentFrames.count >= 5, let first = recentFrames.first, now - first >= 0.4 {
                parts.append("近 2 秒收到约 \(Int((Double(recentFrames.count - 1) / (now - first)).rounded())) fps")
            }
        }
        if let resolutionNotice = resolutionNotice { parts.append(resolutionNotice) }
        if let inputNotice = inputNotice { parts.append(inputNotice) }
        if !hiddenSourceView.isHidden { parts.append("源端窗口已最小化或隐藏 · 可恢复") }
        window.subtitle = parts.joined(separator: " · ")
    }
    func present(_ image: CGImage, mediaMode: String) {
        present(VideoFrame(image: image), mediaMode: mediaMode)
    }
    func present(_ image: VideoFrame, mediaMode: String) {
        surface.displayFrame = image; self.mediaMode = mediaMode
        hasReceivedFrame = true
        recentFrames.append(ProcessInfo.processInfo.systemUptime)
        updateSubtitle()
    }
    func windowShouldClose(_ sender: NSWindow) -> Bool {
        if allowClose { return true }
        guard !closeRequested else { return false }
        closeRequested = true
        resizeTimer?.invalidate(); resizeTimer = nil
        cancelResolutionReconciliation()
        surface.releaseInput()
        // A native close only detaches this controller. The user can still
        // close the source application from inside its own window.
        disconnect()
        return false
    }
    func windowDidBecomeKey(_ notification: Notification) {
        guard remoteVisible else { return }
        transmit(["type": "window.focus", "window_id": id])
    }
    func windowDidResignKey(_ notification: Notification) { surface.releaseInput() }
    func windowDidResize(_ notification: Notification) {
        if !applyingRemote { onLayoutChange?() }
        guard !fixedCanvas, !applyingRemote, remoteVisible, !popup, resizeTimer == nil else { return }
        // Common modes keep this running during AppKit's live resize loop.
        // Coalesce to the latest bounds without postponing every update.
        let timer = Timer(timeInterval: 0.10, repeats: false) { [weak self] _ in self?.sendResize() }
        resizeTimer = timer
        RunLoop.main.add(timer, forMode: .common)
    }
    func windowDidMove(_ notification: Notification) { if !applyingRemote { onLayoutChange?() } }
    func windowDidMiniaturize(_ notification: Notification) { onLayoutChange?() }
    func windowDidDeminiaturize(_ notification: Notification) { onLayoutChange?() }
    func windowDidEndLiveResize(_ notification: Notification) { sendResize() }
    func windowDidChangeBackingProperties(_ notification: Notification) { windowDidResize(notification) }
    func windowDidChangeScreen(_ notification: Notification) {
        onScreenChange?()
        if !applyingRemote { onLayoutChange?() }
    }
    private func sendResize(reconciling: Bool = false) {
        resizeTimer?.invalidate(); resizeTimer = nil
        cancelResolutionReconciliation()
        guard !fixedCanvas, !applyingRemote, remoteVisible, !popup, !closeRequested, !allowClose else { return }
        if reconciling {
            guard !relatedWindow, resolutionNotice == nil, reconciliationAttempts > 0 else { return }
            reconciliationAttempts -= 1
        } else { reconciliationAttempts = 2 }
        guard let pixels = fixedPixels ?? rendering.pixels(forLogicalSize: surface.bounds.size) else { return }
        requestedPixels = pixels
        resolutionNotice = nil
        resolutionTimer?.invalidate(); resolutionDelayed = false
        updateSubtitle()
        if matchesRequestedSize(width: surface.remoteWidth, height: surface.remoteHeight) { return }
        transmit(["type": "window.configure", "window_id": id, "width": pixels.width, "height": pixels.height])
        let timer = Timer(timeInterval: 1.5, repeats: false) { [weak self] _ in
            self?.resolutionDelayed = true; self?.updateSubtitle()
        }
        resolutionTimer = timer; RunLoop.main.add(timer, forMode: .common)
        // A rejected/no-op configure need not generate another geometry event.
        reconcileResolution()
    }
    func useAutomaticResolution() {
        fixedPixels = nil
        windowDidResize(Notification(name: NSWindow.didResizeNotification))
    }
    func showActualPixels() {
        guard let image = surface.displayFrame else { return }
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
    func closeLocally() {
        cancelResolutionReconciliation()
        resizeTimer?.invalidate(); resolutionTimer?.invalidate(); allowClose = true; surface.transmit = nil
        if let scroll = overflowViewport {
            NotificationCenter.default.removeObserver(self, name: NSView.boundsDidChangeNotification, object: scroll.contentView)
        }
        window.parent?.removeChildWindow(window); window.close()
    }
}
