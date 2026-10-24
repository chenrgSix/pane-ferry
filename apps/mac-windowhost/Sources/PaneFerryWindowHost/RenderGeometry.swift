import Foundation

/// Keep source-sized overlay content readable while constraining its viewport
/// to the receiving display's work area (which excludes the Dock/menu bar).
struct OverlayViewport {
    let frame: CGRect
    let documentSize: CGSize
    var scrolls: Bool { documentSize.width > frame.width || documentSize.height > frame.height }
    init(content: CGRect, available: CGRect) {
        documentSize = content.size
        let width = min(content.width, available.width)
        let height = min(content.height, available.height)
        frame = CGRect(x: min(max(content.minX, available.minX), available.maxX - width),
                       y: min(max(content.maxY - height, available.minY), available.maxY - height),
                       width: width, height: height)
    }
}

/// Source scene coordinates are independent of decoded pixel density (including
/// fractional Wayland scaling). Legacy peers use their existing x/y pixel space.
struct WindowPresentation {
    let rect: CGRect
    let kind: String
    let order: Int
    var passive: Bool { kind == "tooltip" || kind == "passive" }
    var overlay: Bool { passive || kind == "menu" || kind == "popup" }
    static func parse(_ object: [String: Any], width: Int, height: Int) -> WindowPresentation? {
        let role = object["role"] as? String ?? "toplevel"
        let fallback = role == "popup" ? "popup" : role == "dialog" ? "dialog" : "window"
        let value = object["presentation"] as? [String: Any]
        if object["presentation"] != nil && value == nil { return nil }
        let kind = value?["kind"] as? String ?? fallback
        guard ["window", "dialog", "popup", "menu", "tooltip", "passive"].contains(kind) else { return nil }
        if let value = value, ["x", "y", "width", "height"].contains(where: { value[$0] as? NSNumber == nil }) { return nil }
        let x = ((value?["x"] ?? object["x"]) as? NSNumber)?.doubleValue ?? 0
        let y = ((value?["y"] ?? object["y"]) as? NSNumber)?.doubleValue ?? 0
        let w = (value?["width"] as? NSNumber)?.doubleValue ?? Double(width)
        let h = (value?["height"] as? NSNumber)?.doubleValue ?? Double(height)
        guard [x, y, w, h].allSatisfy({ $0.isFinite }), abs(x) <= 10_000_000, abs(y) <= 10_000_000,
              w > 0, h > 0, w <= 32768, h <= 32768 else { return nil }
        return WindowPresentation(rect: CGRect(x: x, y: y, width: w, height: h), kind: kind,
                                  order: value?["order"] as? Int ?? 0)
    }
    func relativeRect(to parent: WindowPresentation, in imageRect: CGRect) -> CGRect {
        let sx = imageRect.width / parent.rect.width, sy = imageRect.height / parent.rect.height
        return CGRect(x: imageRect.minX + (rect.minX - parent.rect.minX) * sx,
                      y: imageRect.minY + (rect.minY - parent.rect.minY) * sy,
                      width: rect.width * sx, height: rect.height * sy)
    }
}

/// The wire uses source pixels; AppKit uses logical points. Preserve the
/// source's existing scale, or an explicitly negotiated page presentation
/// density, never the Mac's backing density. Ordinary resizing does not change DPI.
struct RenderGeometry {
    let scale: Int
    init?(scale: Int) {
        guard scale == 1 || scale == 2 else { return nil }
        self.scale = scale
    }
    func points(width: Int, height: Int) -> CGSize {
        CGSize(width: Double(width) / Double(scale), height: Double(height) / Double(scale))
    }
    func pixels(forLogicalSize points: CGSize) -> (width: Int, height: Int)? {
        guard points.width.isFinite, points.height.isFinite, points.width > 0, points.height > 0 else { return nil }
        let density = CGFloat(scale)
        let width = max(64, (points.width * density).rounded())
        let height = max(48, (points.height * density).rounded())
        guard width.isFinite, height.isFinite else { return nil }
        // Keep the viewport ratio even when a large display exceeds the frame budget.
        let fit = min(1, 4096 / width, 2160 / height, sqrt(8_000_000 / width / height))
        return (max(1, Int((width * fit).rounded(.down))), max(1, Int((height * fit).rounded(.down))))
    }
    static func imageRect(in bounds: CGRect, width: Int, height: Int) -> CGRect {
        guard width > 0, height > 0, bounds.width > 0, bounds.height > 0 else { return .zero }
        let fit = min(bounds.width / Double(width), bounds.height / Double(height))
        let size = CGSize(width: Double(width) * fit, height: Double(height) * fit)
        return CGRect(x: bounds.midX - size.width / 2, y: bounds.midY - size.height / 2, width: size.width, height: size.height)
    }
}
