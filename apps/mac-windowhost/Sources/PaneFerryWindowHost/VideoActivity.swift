import Foundation

enum VideoActivity: String {
    case foreground, background, suspended

    static func select(_ windows: [(visible: Bool, focused: Bool)]) -> VideoActivity {
        if windows.contains(where: { $0.visible && $0.focused }) { return .foreground }
        if windows.contains(where: { $0.visible }) { return .background }
        return .suspended
    }

    func fallbackFPS(_ requested: Int) -> Int {
        switch self {
        case .foreground: return requested
        case .background: return min(requested, 5)
        case .suspended: return 1 // Legacy publishers have no pause capability.
        }
    }
}
