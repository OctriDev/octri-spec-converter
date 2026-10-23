import Foundation

#if canImport(Darwin)
  import Darwin
#else
  import Glibc
#endif

public struct ConversionError: Error, Sendable {
  public let code: String
  public let message: String
  public let diagnostics: JSONValue
}

/// The C ABI entry points of one loaded library. The library is never unloaded,
/// so the function pointers stay valid for the life of the process.
private struct NativeLibrary: @unchecked Sendable {
  let abiVersion: @convention(c) () -> UInt32
  let call: @convention(c) (UnsafePointer<UInt8>?, Int) -> OpaquePointer?
  let data: @convention(c) (OpaquePointer?) -> UnsafeMutablePointer<UInt8>?
  let length: @convention(c) (OpaquePointer?) -> Int
  let free: @convention(c) (OpaquePointer?) -> Void

  init(path: String) throws {
    guard let handle = dlopen(path, RTLD_NOW | RTLD_LOCAL) else {
      let reason = dlerror().map { String(cString: $0) } ?? path
      throw ConversionError(
        code: "NATIVE_LIBRARY", message: "Cannot load native library: \(reason)",
        diagnostics: .array([]))
    }
    func symbol<T>(_ name: String) throws -> T {
      guard let address = dlsym(handle, name) else {
        throw ConversionError(
          code: "NATIVE_LIBRARY", message: "Native library lacks \(name)", diagnostics: .array([]))
      }
      return unsafeBitCast(address, to: T.self)
    }
    abiVersion = try symbol("octri_converter_abi_version")
    call = try symbol("octri_converter_call")
    data = try symbol("octri_converter_buffer_data")
    length = try symbol("octri_converter_buffer_len")
    free = try symbol("octri_converter_buffer_free")
  }

  /// The library this package bundles for the current platform.
  static func bundled() throws -> String {
    #if os(macOS) && arch(arm64)
      let (platform, file) = ("darwin-arm64", "liboctri_converter_ffi.dylib")
    #elseif os(Linux) && arch(x86_64)
      let (platform, file) = ("linux-x64", "liboctri_converter_ffi.so")
    #elseif os(Linux) && arch(arm64)
      let (platform, file) = ("linux-arm64", "liboctri_converter_ffi.so")
    #else
      throw ConversionError(
        code: "NATIVE_LIBRARY",
        message: "No bundled native library for this platform; pass its path",
        diagnostics: .array([]))
    #endif
    guard
      let url = Bundle.module.url(
        forResource: file, withExtension: nil, subdirectory: "native/\(platform)")
    else {
      throw ConversionError(
        code: "NATIVE_LIBRARY",
        message: "This build carries no native library for \(platform); pass its path",
        diagnostics: .array([]))
    }
    return url.path
  }
}

/// Synchronous, stateless facade. Call from a background task for large inputs.
public struct Converter: Sendable {
  private let native: NativeLibrary

  /// Loads the native library bundled with this package for the current platform.
  public init() throws {
    try self.init(library: NativeLibrary.bundled())
  }

  public init(library: String) throws {
    native = try NativeLibrary(path: library)
    guard native.abiVersion() == 1 else {
      throw ConversionError(
        code: "ABI_VERSION", message: "Expected converter ABI version 1", diagnostics: .array([]))
    }
  }

  public func call(_ envelope: JSONValue) throws -> JSONValue {
    let input = try envelope.encoded()
    guard input.count <= 128 * 1024 * 1024 else {
      throw ConversionError(
        code: "LIMIT_REQUEST_BYTES", message: "Converter envelope exceeds 128 MiB",
        diagnostics: .array([]))
    }
    let output = input.withUnsafeBytes { bytes in
      native.call(bytes.baseAddress?.assumingMemoryBound(to: UInt8.self), bytes.count)
    }
    guard let output else {
      throw ConversionError(
        code: "ALLOCATION", message: "Native converter returned a null buffer",
        diagnostics: .array([]))
    }
    defer { native.free(output) }
    let count = native.length(output)
    guard let bytes = native.data(output) else { throw JSONTransportError.invalid }
    return try JSONValue.decode(Data(bytes: bytes, count: count))
  }

  private func result(_ action: String, _ fields: [JSONKey: JSONValue] = [:]) throws -> JSONValue {
    var envelope = fields
    envelope["abiVersion"] = .integer(1)
    envelope["action"] = .string(action)
    let response = try call(.object(envelope))
    if let error = response["Err"] {
      let code: String
      let message: String
      if case .string(let value) = error["code"] { code = value } else { code = "NATIVE_ERROR" }
      if case .string(let value) = error["message"] {
        message = value
      } else {
        message = "Native convert failed"
      }
      throw ConversionError(
        code: code, message: message, diagnostics: error["diagnostics"] ?? .array([]))
    }
    guard let value = response["Ok"] else { throw JSONTransportError.invalid }
    return value
  }

  public func convert(_ request: JSONValue) throws -> JSONValue {
    try result("convert", ["request": request])
  }
  public func review(_ request: JSONValue) throws -> JSONValue {
    try result("review", ["request": request])
  }
  public func bundle(_ request: JSONValue) throws -> JSONValue {
    try result("bundle", ["request": request])
  }
  public func compare(_ before: JSONValue, _ after: JSONValue) throws -> JSONValue {
    try result("compare", ["before": before, "after": after])
  }
  public func export(_ report: JSONValue, format: String = "json") throws -> JSONValue {
    try result("export", ["report": report, "format": .string(format)])
  }
  public func formats() throws -> JSONValue { try result("formats") }
  public func detect(_ request: JSONValue) throws -> JSONValue {
    try result("detect", ["request": request])
  }
  public func batch(_ requests: [JSONValue]) throws -> JSONValue {
    try result("batch", ["requests": .array(requests)])
  }
  public static func text(_ content: String, uri: String = "memory://convert/openapi.yaml")
    -> JSONValue
  {
    .object(["uri": .string(uri), "content": .string(content), "generated": .bool(false)])
  }
}
