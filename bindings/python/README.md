# Octri Spec Converter for Python

[Live demo](https://octri.dev/spec-converter) · Convert Swagger, OpenAPI, Postman, Insomnia, RAML,
WSDL and cURL descriptions to OpenAPI, offline, with Octri's Rust converter.

```sh
pip install octri-spec-converter
```

```python
from octri_converter import Converter, text

report = Converter().convert({"input": text(source_text), "config": {"targetVersion": "3.2.1"}})
print(report["output"]["content"])
```

Each platform wheel bundles the native library.
Supported platforms: Linux x64 and arm64 (glibc 2.36 or later) and macOS arm64. On other platforms, build
`liboctri_converter_ffi` from [the repository](https://github.com/octridev/octri-spec-converter) and pass its path to `Converter`.

Requests, reports and the runtime contract are documented in the
[bindings guide](https://github.com/octridev/octri-spec-converter/blob/main/bindings/README.md). Licensed under Apache-2.0; the bundled library's
third-party notices ship with the package.
