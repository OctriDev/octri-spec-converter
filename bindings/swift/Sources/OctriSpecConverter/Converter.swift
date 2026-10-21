import COctriConverter
import Foundation

public struct ConversionError: Error, Sendable {
  public let code: String
  public let message: String
  public let diagnostics: JSONValue
}

/// Synchronous, stateless facade. Call from a background task for large inputs.
public struct Converter: Sendable {
  public init() throws {
    guard octri_converter_abi_version() == 1 else {
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
      octri_converter_call(bytes.baseAddress?.assumingMemoryBound(to: UInt8.self), bytes.count)
    }
    guard let output else {
      throw ConversionError(
        code: "ALLOCATION", message: "Native converter returned a null buffer",
        diagnostics: .array([]))
    }
    defer { octri_converter_buffer_free(output) }
    let count = octri_converter_buffer_len(output)
    guard let bytes = octri_converter_buffer_data(output) else { throw JSONTransportError.invalid }
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
