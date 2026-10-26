// swift-tools-version:5.9

import PackageDescription

let package = Package(
    name: "MalacKit",
    platforms: [.iOS(.v17), .macOS(.v14)],
    products: [
        .library(name: "MalacKit", targets: ["MalacKit"]),
    ],
    targets: [
        .target(name: "MalacKit"),
        .testTarget(name: "MalacKitTests", dependencies: ["MalacKit"]),
    ]
)
