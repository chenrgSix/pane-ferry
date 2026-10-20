// swift-tools-version: 5.9
import PackageDescription
let package = Package(
    name: "RemoteAppWindowHost",
    platforms: [.macOS(.v12)],
    products: [.executable(name: "RemoteAppWindowHost", targets: ["RemoteAppWindowHost"])],
    targets: [.executableTarget(name: "RemoteAppWindowHost")]
)
