import Foundation

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
