import Foundation

/// Wire dimensions are physical pixels; AppKit window dimensions are points.
struct RenderGeometry {
    let scale: Int
    init?(scale: Int) {
        guard scale == 1 || scale == 2 else { return nil }
        self.scale = scale
    }
    func points(width: Int, height: Int) -> CGSize {
        CGSize(width: Double(width) / Double(scale), height: Double(height) / Double(scale))
    }
    func pixels(for points: CGSize) -> (width: Int, height: Int)? {
        guard points.width.isFinite, points.height.isFinite, points.width > 0, points.height > 0 else { return nil }
        let width = min(4096, max(64, (points.width * Double(scale)).rounded()))
        let height = min(2160, max(48, (points.height * Double(scale)).rounded()))
        let fit = min(1, sqrt(8_000_000 / (width * height)))
        return (Int((width * fit).rounded(.down)), Int((height * fit).rounded(.down)))
    }
    static func imageRect(in bounds: CGRect, width: Int, height: Int) -> CGRect {
        guard width > 0, height > 0, bounds.width > 0, bounds.height > 0 else { return .zero }
        let fit = min(bounds.width / Double(width), bounds.height / Double(height))
        let size = CGSize(width: Double(width) * fit, height: Double(height) * fit)
        return CGRect(x: bounds.midX - size.width / 2, y: bounds.midY - size.height / 2, width: size.width, height: size.height)
    }
}
