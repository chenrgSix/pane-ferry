import Foundation

enum DisplayQuality: String, CaseIterable {
    case clarity = "png"
    case smooth = "h264"

    var title: String {
        switch self {
        case .clarity: return "清晰优先（PNG 无损）"
        case .smooth: return "流畅优先（H.264）"
        }
    }
}

/// Named application bundles have different identifiers. Use one Mac-only
/// preference for future connections, without changing other running sessions.
struct DisplayQualitySettings {
    let defaults: UserDefaults
    init(defaults: UserDefaults = UserDefaults(suiteName: "io.paneferry.display")!) {
        self.defaults = defaults
    }
    var selected: DisplayQuality {
        get { defaults.string(forKey: "quality").flatMap(DisplayQuality.init(rawValue:)) ?? .clarity }
        nonmutating set { defaults.set(newValue.rawValue, forKey: "quality") }
    }
}
