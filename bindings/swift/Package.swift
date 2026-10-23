// swift-tools-version: 6.0
import PackageDescription

let package = Package(
  name: "OctriSpecConverter",
  platforms: [.macOS(.v13)],
  products: [.library(name: "OctriSpecConverter", targets: ["OctriSpecConverter"])],
  targets: [
    // Release builds place each platform's native library under native/.
    .target(name: "OctriSpecConverter", resources: [.copy("native")]),
    .executableTarget(
      name: "BindingCheck", dependencies: ["OctriSpecConverter"], path: "Tests/BindingCheck"),
  ]
)
