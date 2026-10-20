import Foundation

/// A remote clipboard reply may update the controller only after its own explicit copy request.
struct ClipboardCopyGate {
    private var pending: (id: String, window: UInt32, deadline: Date)?
    mutating func begin(id: String, window: UInt32, now: Date = Date()) {
        pending = (id, window, now.addingTimeInterval(5))
    }
    mutating func accept(id: String, window: UInt32, text: String, now: Date = Date()) -> String? {
        guard let request = pending, request.deadline > now, request.id == id, request.window == window else { return nil }
        pending = nil
        guard !text.isEmpty, text.utf8.count <= controllerTextLimit, !text.contains("\0") else { return nil }
        return text
    }
}
