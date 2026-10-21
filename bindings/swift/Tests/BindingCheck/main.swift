import Foundation
import OctriSpecConverter

let envelope = try JSONValue.decode(FileHandle.standardInput.readDataToEndOfFile())
let converter = try Converter()
let response: JSONValue
if ProcessInfo.processInfo.environment["OCTRI_BINDING_RAW"] != "1", envelope["abiVersion"] == .integer(1), let action = envelope["action"],
   [.string("convert"), .string("review"), .string("bundle"), .string("compare"), .string("export"), .string("formats"), .string("detect"), .string("batch")].contains(action) {
  let value: JSONValue
  switch action {
  case .string("convert"): value = try converter.convert(envelope["request"]!)
  case .string("review"): value = try converter.review(envelope["request"]!)
  case .string("bundle"): value = try converter.bundle(envelope["request"]!)
  case .string("detect"): value = try converter.detect(envelope["request"]!)
  case .string("batch"):
    guard case .array(let requests) = envelope["requests"] else { fatalError("Missing batch requests") }
    value = try converter.batch(requests)
  case .string("compare"): value = try converter.compare(envelope["before"]!, envelope["after"]!)
  case .string("export"):
    guard case .string(let format) = envelope["format"] else { fatalError("Missing export format") }
    value = try converter.export(envelope["report"]!, format: format)
  default: value = try converter.formats()
  }
  response = .object(["Ok": value])
} else {
  response = try converter.call(envelope)
}
FileHandle.standardOutput.write(try response.encoded())
