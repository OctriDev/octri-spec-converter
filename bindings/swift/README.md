# Octri Spec Converter for Swift

[Live demo](https://octri.dev/spec-converter) · Convert Swagger, OpenAPI, Postman, Insomnia, RAML,
WSDL and cURL descriptions to OpenAPI, offline, with Octri's Rust converter.

```sh
# Package.swift
.package(url: "https://github.com/octridev/octri-spec-converter-swift", from: "0.1.3")
```

```swift
import OctriSpecConverter

let report = try Converter().convert(.object(["input": Converter.text(sourceText)]))
```

The package bundles the native library as a resource.
Supported platforms: Linux x64 and arm64 (glibc 2.36 or later) and macOS arm64. On other platforms, build
`liboctri_converter_ffi` from [the repository](https://github.com/octridev/octri-spec-converter) and pass its path to `Converter(library:)`.

macOS consumers declare macOS 13 or later. Linux executables should link with lld and separate code segments:
`-Xswiftc -use-ld=lld -Xlinker -z -Xlinker separate-code`.

Requests, reports and the runtime contract are documented in the
[bindings guide](https://github.com/octridev/octri-spec-converter/blob/main/bindings/README.md). Licensed under Apache-2.0; the bundled library's
third-party notices ship with the package.
