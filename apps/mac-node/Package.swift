// swift-tools-version: 5.9
import PackageDescription
let package = Package(name: "RemoteAppNode", platforms: [.macOS(.v13)],
    products: [.executable(name: "RemoteAppNode", targets: ["RemoteAppNode"])],
    targets: [.executableTarget(name: "RemoteAppNode")])
