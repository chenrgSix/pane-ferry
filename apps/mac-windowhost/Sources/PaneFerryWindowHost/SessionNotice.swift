import AppKit

/// A session owns at most one modeless notice. Remote errors must never enter
/// a nested AppKit modal loop or prevent another session from being closed.
final class SessionNotice: NSObject, NSWindowDelegate {
    private(set) var current: NSAlert?
    private var shownKeys: [String] = []
    private var onDismiss: (() -> Void)?
    private var onDisconnect: (() -> Void)?

    func resetSuppression() { shownKeys.removeAll() }

    func present(title: String, message: String, icon: NSImage?, parent: NSWindow?,
                 key: String? = nil, dismissed: (() -> Void)? = nil,
                 disconnect: (() -> Void)? = nil) {
        if let key = key {
            let bounded = String(key.prefix(160))
            guard !shownKeys.contains(bounded) else { return }
            shownKeys.append(bounded)
            if shownKeys.count > 32 { shownKeys.removeFirst() }
        }
        // A different error replaces the current notice instead of queueing
        // dialogs that the user would have to dismiss one by one.
        dismiss()
        let notice = NSAlert()
        notice.messageText = title; notice.informativeText = message
        if let icon = icon { notice.icon = icon }
        let acknowledge = notice.addButton(withTitle: "好")
        acknowledge.target = self; acknowledge.action = #selector(acknowledgeNotice)
        if disconnect != nil {
            let button = notice.addButton(withTitle: "断开会话")
            button.target = self; button.action = #selector(disconnectNotice)
        }
        notice.layout()
        let window = notice.window
        window.title = title; window.styleMask.insert(.closable)
        window.isReleasedWhenClosed = false; window.isExcludedFromWindowsMenu = true
        window.level = .normal; window.delegate = self
        current = notice; onDismiss = dismissed; onDisconnect = disconnect
        if let parent = parent {
            parent.addChildWindow(window, ordered: .above)
            window.setFrameOrigin(NSPoint(x: parent.frame.midX - window.frame.width / 2,
                                          y: parent.frame.midY - window.frame.height / 2))
        } else { window.center() }
        window.makeKeyAndOrderFront(nil)
    }

    /// Lifecycle cleanup does not invoke a user action or start another detach.
    func dismiss() {
        let window = current?.window
        current = nil; onDismiss = nil; onDisconnect = nil
        window?.delegate = nil
        if let window = window { window.parent?.removeChildWindow(window) }
        window?.close()
    }

    @objc private func acknowledgeNotice() {
        let completion = onDismiss
        dismiss(); completion?()
    }
    @objc private func disconnectNotice() {
        let completion = onDisconnect
        dismiss(); completion?()
    }
    func windowShouldClose(_ sender: NSWindow) -> Bool {
        acknowledgeNotice()
        return false
    }
}
