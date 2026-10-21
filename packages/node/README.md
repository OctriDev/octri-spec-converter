# Octri Spec Converter — Node.js

Convert supported Swagger/OpenAPI, Postman, RAML, Insomnia, HAR, cURL and mapped protocol specifications to OpenAPI, with explicit review of incompatible or lossy changes.

Part of [octri-spec-converter](https://github.com/octridev/octri-spec-converter), maintained by [OctriDev](https://github.com/octridev). Licensed under Apache-2.0. Bundled dependencies retain their own licenses and notices.

## Install

```sh
npm install @octri/spec-converter-native
```

## Example

```js
import { Converter, text } from "@octri/spec-converter-native";

const report = await Converter.convert({
  input: text(
    JSON.stringify({
      swagger: "2.0",
      info: { title: "Example API", version: "1.0.0" },
      paths: {},
    }),
  ),
  config: {},
  files: {},
});
console.log(report.valid, report.document);
```

## Runtime and support

Requires Node.js 22.19 or later and a native addon matching your operating system and architecture. Published binaries cover macOS ARM64 and Linux x64/ARM64 with glibc 2.36 or later (Debian 12, Ubuntu 22.10 or newer). Alpine/musl and older glibc systems must use WASM or build from source. Native binaries and platform availability are separate from the portable WASM package. Apply process deadlines and memory limits when serving untrusted documents.

See [supported formats and limits](https://github.com/octridev/octri-spec-converter#supported-formats) and [language bindings](https://github.com/octridev/octri-spec-converter/blob/main/bindings/README.md). Non-HTTP protocols require explicit HTTP mappings. Unsupported constructs remain errors, blockers or reviewed omissions.

Browser and package wrappers may receive packaging-only patch releases while the embedded engine version stays unchanged. Inspect result/asset manifests for the engine version.

Issues and contributions: [octri-spec-converter](https://github.com/octridev/octri-spec-converter). Report security issues using [GitHub private vulnerability reporting](https://github.com/octridev/octri-spec-converter/security/advisories/new).
