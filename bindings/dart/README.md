# Octri Spec Converter for Dart

[Live demo](https://octri.dev/spec-converter) · Convert Swagger, OpenAPI, Postman, Insomnia, RAML,
WSDL and cURL descriptions to OpenAPI, offline, with Octri's Rust converter.

```sh
dart pub add octri_spec_converter
```

```dart
import 'package:octri_spec_converter/octri_spec_converter.dart';

final report = Converter().convert({'input': Converter.text(sourceText)});
```

The package bundles the native library.
Supported platforms: Linux x64 and arm64 (glibc 2.36 or later) and macOS arm64. On other platforms, build
`liboctri_converter_ffi` from [the repository](https://github.com/octridev/octri-spec-converter) and pass its path to `Converter`.

Runs on the Dart VM. A `dart compile exe` binary cannot read package files: ship the library beside it and pass its path to `Converter`.

Requests, reports and the runtime contract are documented in the
[bindings guide](https://github.com/octridev/octri-spec-converter/blob/main/bindings/README.md). Licensed under Apache-2.0; the bundled library's
third-party notices ship with the package.
