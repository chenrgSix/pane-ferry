import Foundation

/// Only a matching explicit copy/cut reply may update the controller clipboard.
struct ClipboardCopyGate {
    private var pending: (id: String, window: UInt32, deadline: Date)?
    mutating func begin(id: String, window: UInt32, now: Date = Date(), timeout: TimeInterval = 5) {
        pending = (id, window, now.addingTimeInterval(timeout))
    }
    mutating func claim(id: String, window: UInt32, now: Date = Date()) -> Bool {
        guard let request = pending, request.deadline > now, request.id == id, request.window == window else { return false }
        pending = nil; return true
    }
    mutating func accept(id: String, window: UInt32, text: String, now: Date = Date()) -> String? {
        guard claim(id: id, window: window, now: now) else { return nil }
        guard !text.isEmpty, text.utf8.count <= controllerTextLimit, !text.contains("\0") else { return nil }
        return text
    }
}
