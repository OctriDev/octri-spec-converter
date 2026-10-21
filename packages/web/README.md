# Octri Spec Converter Web

Convert supported Swagger/OpenAPI, Postman, RAML, Insomnia, HAR, cURL and mapped protocol specifications to OpenAPI, with explicit review of incompatible or lossy changes.

Part of [octri-spec-converter](https://github.com/octridev/octri-spec-converter), maintained by [OctriDev](https://github.com/octridev). Licensed under Apache-2.0. Bundled dependencies retain their own licenses and notices.

## Install

```sh
npm install @octri/spec-converter-web
```

## Host the browser product

Serve the installed package directory using your static host. Open `web/index.html` within that directory. Keep `web/`, `packages/` and `vendor/` in their published relative layout; workers and WASM use those paths. Serve `.wasm` as `application/wasm` and JavaScript modules with a JavaScript MIME type.

`manifest.json` records SHA-256 hashes for every runtime asset. Verify those hashes before copying or serving the product. The package includes its Rust-compiled WASM engine, worker code, HTML/CSS and legal notices. Conversion or auditing runs locally in browser workers; this package does not require an Octri account or hosted Octri API. Optional URL retrieval and application handoff require an explicitly configured host integration.

## Runtime and support

See [supported formats and limits](https://github.com/octridev/octri-spec-converter#supported-formats) and [language bindings](https://github.com/octridev/octri-spec-converter/blob/main/bindings/README.md). Non-HTTP protocols require explicit HTTP mappings. Unsupported constructs remain errors, blockers or reviewed omissions.

Browser and package wrappers may receive packaging-only patch releases while the embedded engine version stays unchanged. Inspect result/asset manifests for the engine version.

Issues and contributions: [octri-spec-converter](https://github.com/octridev/octri-spec-converter). Report security issues using [GitHub private vulnerability reporting](https://github.com/octridev/octri-spec-converter/security/advisories/new).
