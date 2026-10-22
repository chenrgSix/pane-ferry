// swift-tools-version: 5.9
import PackageDescription
let package = Package(name: "PaneFerryNode", platforms: [.macOS(.v13)],
    products: [.executable(name: "PaneFerryNode", targets: ["PaneFerryNode"])],
    targets: [.executableTarget(name: "PaneFerryNode")])
