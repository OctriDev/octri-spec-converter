# Octri Spec Converter for Ruby

[Live demo](https://octri.dev/spec-converter) · Convert Swagger, OpenAPI, Postman, Insomnia, RAML,
WSDL and cURL descriptions to OpenAPI, offline, with Octri's Rust converter.

```sh
gem install octri-spec-converter
```

```ruby
require 'octri_converter'

report = OctriConverter::Converter.new.convert('input' => OctriConverter.text(source_text))
```

Each platform gem bundles the native library.
Supported platforms: Linux x64 and arm64 (glibc 2.36 or later) and macOS arm64. On other platforms, build
`liboctri_converter_ffi` from [the repository](https://github.com/octridev/octri-spec-converter) and pass its path to `Converter.new`.

Requests, reports and the runtime contract are documented in the
[bindings guide](https://github.com/octridev/octri-spec-converter/blob/main/bindings/README.md). Licensed under Apache-2.0; the bundled library's
third-party notices ship with the package.
