# Octri Spec Converter for Go

[Live demo](https://octri.dev/spec-converter) · Convert Swagger, OpenAPI, Postman, Insomnia, RAML,
WSDL and cURL descriptions to OpenAPI, offline, with Octri's Rust converter.

```sh
go get github.com/octridev/octri-spec-converter-go
```

```go
import converter "github.com/octridev/octri-spec-converter-go"

report, err := converter.Converter{}.Convert(converter.Request{Input: converter.Text(sourceText, "memory://convert/openapi.yaml")})
```

The module bundles a static library.
Supported platforms: Linux x64 and arm64 (glibc 2.36 or later) and macOS arm64. On other platforms, build
`liboctri_converter_ffi` from [the repository](https://github.com/octridev/octri-spec-converter) and add its directory to `CGO_LDFLAGS` with `-L`.

Build with `CGO_ENABLED=1` and a C compiler. The library links statically, so binaries need nothing installed beside them.

Requests, reports and the runtime contract are documented in the
[bindings guide](https://github.com/octridev/octri-spec-converter/blob/main/bindings/README.md). Licensed under Apache-2.0; the bundled library's
third-party notices ship with the package.
