import Foundation

/// Swift String equality folds canonical Unicode equivalents. JSON property
/// names and JSON Pointers do not. Hash/compare their original UTF-8 instead.
public struct JSONKey: Hashable, Sendable, ExpressibleByStringLiteral {
  public let text: String
  public init(_ text: String) { self.text = text }
  public init(stringLiteral value: String) { text = value }
  public static func == (lhs: Self, rhs: Self) -> Bool {
    lhs.text.utf8.elementsEqual(rhs.text.utf8)
  }
  public func hash(into hasher: inout Hasher) {
    if text.utf8.withContiguousStorageIfAvailable({ bytes in
      hasher.combine(bytes: UnsafeRawBufferPointer(bytes))
    }) == nil {
      for byte in text.utf8 { hasher.combine(byte) }
    }
  }
}

/// Lossless JSON structure, including byte-distinct Unicode keys and arbitrary decimal tokens.
/// This is transport decoding only; all convert semantics remain in Rust.
public indirect enum JSONValue: Sendable, Equatable {
  case object([JSONKey: JSONValue])
  case array([JSONValue])
  case string(String)
  case integer(Int64)
  case unsigned(UInt64)
  case number(Double)
  /// Exact JSON token. Encoding validates its grammar before emitting bytes.
  case decimal(String)
  case bool(Bool)
  case null

  public subscript(key: String) -> JSONValue? {
    guard case .object(let values) = self else { return nil }
    return values[JSONKey(key)]
  }

  public static func decode(_ data: Data) throws -> JSONValue {
    var reader = JSONReader(bytes: Array(data))
    let value = try reader.value(depth: 0)
    reader.whitespace()
    guard reader.offset == reader.bytes.count else { throw JSONTransportError.invalid }
    return value
  }

  public func encoded() throws -> Data {
    var output = Data()
    try write(into: &output, depth: 0)
    return output
  }

  private func write(into output: inout Data, depth: Int) throws {
    guard depth <= 512 else { throw JSONTransportError.depth }
    switch self {
    case .object(let fields):
      output.append(123)
      let keys = fields.keys.sorted { $0.text.utf8.lexicographicallyPrecedes($1.text.utf8) }
      for (index, key) in keys.enumerated() {
        if index != 0 { output.append(44) }
        output.append(try JSONEncoder().encode(key.text))
        output.append(58)
        if let value = fields[key] { try value.write(into: &output, depth: depth + 1) }
      }
      output.append(125)
    case .array(let values):
      output.append(91)
      for (index, value) in values.enumerated() {
        if index != 0 { output.append(44) }
        try value.write(into: &output, depth: depth + 1)
      }
      output.append(93)
    case .string(let value): output.append(try JSONEncoder().encode(value))
    case .integer(let value): output.append(contentsOf: String(value).utf8)
    case .unsigned(let value): output.append(contentsOf: String(value).utf8)
    case .number(let value): output.append(try JSONEncoder().encode(value))
    case .decimal(let token):
      let pattern = #"^-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?$"#
      guard let range = token.range(of: pattern, options: .regularExpression),
        range == token.startIndex..<token.endIndex
      else { throw JSONTransportError.invalid }
      output.append(contentsOf: token.utf8)
    case .bool(let value): output.append(contentsOf: (value ? "true" : "false").utf8)
    case .null: output.append(contentsOf: "null".utf8)
    }
  }
}

public enum JSONTransportError: Error { case invalid, depth }

// Containers preserve byte-distinct Unicode keys. Plain UTF-8 strings and JSON
// numbers use allocation-light paths; Foundation handles escaped string tokens.
private struct JSONReader {
  let bytes: [UInt8]
  var offset = 0
  var byte: UInt8? { offset < bytes.count ? bytes[offset] : nil }

  mutating func whitespace() {
    while let byte {
      switch byte {
      case 9, 10, 13, 32: offset += 1
      default: return
      }
    }
  }

  mutating func consume(_ expected: UInt8) throws {
    whitespace()
    guard byte == expected else { throw JSONTransportError.invalid }
    offset += 1
  }

  mutating func string() throws -> String {
    try consume(34)
    let start = offset - 1
    var escaped = false
    while let byte {
      offset += 1
      if byte == 34 {
        if !escaped {
          guard let value = String(bytes: bytes[(start + 1)..<(offset - 1)], encoding: .utf8)
          else { throw JSONTransportError.invalid }
          return value
        }
        return try JSONDecoder().decode(String.self, from: Data(bytes[start..<offset]))
      }
      guard byte >= 32 else { throw JSONTransportError.invalid }
      if byte == 92 {
        escaped = true
        guard offset < bytes.count else { throw JSONTransportError.invalid }
        offset += 1
      }
    }
    throw JSONTransportError.invalid
  }

  /// Validate the JSON number grammar before Swift's permissive conversions.
  /// Decimal and exponent tokens never pass through binary floating point.
  mutating func number() throws -> JSONValue {
    let start = offset
    if byte == 45 { offset += 1 }
    if byte == 48 {
      offset += 1
    } else {
      guard let byte, byte >= 49 && byte <= 57 else { throw JSONTransportError.invalid }
      digits()
    }
    if byte == 46 {
      offset += 1
      guard let byte, byte >= 48 && byte <= 57 else { throw JSONTransportError.invalid }
      digits()
    }
    if byte == 69 || byte == 101 {
      offset += 1
      if byte == 43 || byte == 45 { offset += 1 }
      guard let byte, byte >= 48 && byte <= 57 else { throw JSONTransportError.invalid }
      digits()
    }
    let token = String(decoding: bytes[start..<offset], as: UTF8.self)
    if !token.utf8.contains(46) && !token.utf8.contains(69) && !token.utf8.contains(101) {
      if let integer = Int64(token) { return .integer(integer) }
      if let integer = UInt64(token) { return .unsigned(integer) }
    }
    return .decimal(token)
  }

  mutating func digits() {
    while let byte, byte >= 48 && byte <= 57 { offset += 1 }
  }

  mutating func literal(_ expected: [UInt8]) throws {
    guard bytes.count - offset >= expected.count,
      bytes[offset..<(offset + expected.count)].elementsEqual(expected)
    else { throw JSONTransportError.invalid }
    offset += expected.count
  }

  mutating func value(depth: Int) throws -> JSONValue {
    guard depth <= 512 else { throw JSONTransportError.depth }
    whitespace()
    guard let leading = byte else { throw JSONTransportError.invalid }
    switch leading {
    case 123:
      offset += 1
      whitespace()
      var fields: [JSONKey: JSONValue] = [:]
      if byte != 125 {
        while true {
          let key = JSONKey(try string())
          guard fields[key] == nil else { throw JSONTransportError.invalid }
          try consume(58)
          fields[key] = try value(depth: depth + 1)
          whitespace()
          if byte != 44 { break }
          offset += 1
        }
      }
      try consume(125)
      return .object(fields)
    case 91:
      offset += 1
      whitespace()
      var values: [JSONValue] = []
      if byte != 93 {
        while true {
          values.append(try value(depth: depth + 1))
          whitespace()
          if byte != 44 { break }
          offset += 1
        }
      }
      try consume(93)
      return .array(values)
    case 34: return .string(try string())
    case 116:
      try literal([116, 114, 117, 101])
      return .bool(true)
    case 102:
      try literal([102, 97, 108, 115, 101])
      return .bool(false)
    case 110:
      try literal([110, 117, 108, 108])
      return .null
    case 45, 48...57: return try number()
    default: throw JSONTransportError.invalid
    }
  }
}
