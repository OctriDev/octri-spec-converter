# Changelog

## 0.1.3

- Publish the language bindings: PyPI, RubyGems, Maven Central, pub.dev and crates.io packages, Go, PHP and Swift mirror repositories, and C archives on the GitHub release. Each package carries the native library for Linux x64 and arm64 (glibc 2.36+) and macOS arm64, and its no-argument constructor loads it.
- The Swift binding loads the library with `dlopen` instead of linking a system library, and the Go module path is `github.com/octridev/octri-spec-converter-go`. The Ruby gem declares `bigdecimal` and `fiddle`, which leave Ruby's default gems in 3.4 and 4.0.
- Update embedded readiness auditing to ruleset `1.0.3`, preserving authored operation IDs with unfamiliar action verbs and retaining collision-safe duplicate-action repairs.
- Rebuild native bindings and browser assets; npm packages ship as `0.1.6`.

## 0.1.0

- Rust conversion engine for Swagger/OpenAPI, request collections and explicitly
  mapped non-HTTP descriptions, with JSON/YAML targets from Swagger 2.0 through
  OpenAPI 3.2.1.
- Immutable source evidence, physical diagnostics, explicit fidelity review,
  exact numbers and verified report import/export.
- CLI, GitHub Action, browser worker, optional authenticated HTTP host and native
  language bindings using the same engine.
- Explicit Postman credential selection, RAML XML/file facets and owning
  OpenAPI 3.0 reference semantics.

See [support boundaries](README.md) and [deferred coverage](README.md).
