// swift-tools-version: 5.9
import PackageDescription
let package = Package(
    name: "PaneFerryWindowHost",
    platforms: [.macOS(.v12)],
    products: [.executable(name: "PaneFerryWindowHost", targets: ["PaneFerryWindowHost"])],
    targets: [.executableTarget(name: "PaneFerryWindowHost")]
)
