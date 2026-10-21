// swift-tools-version: 6.0
import PackageDescription

let package = Package(
  name: "OctriSpecConverter",
  platforms: [.macOS(.v13)],
  products: [.library(name: "OctriSpecConverter", targets: ["OctriSpecConverter"])],
  targets: [
    .systemLibrary(name: "COctriConverter"),
    .target(name: "OctriSpecConverter", dependencies: ["COctriConverter"]),
    .executableTarget(
      name: "BindingCheck", dependencies: ["OctriSpecConverter"], path: "Tests/BindingCheck"),
  ]
)
