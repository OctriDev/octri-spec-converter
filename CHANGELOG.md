# Changelog

## 0.2.0

- Breaking: `limits` accepts `maxDocumentBytes`, `maxTotalBytes`, `maxNodes`, `maxDepth` and `maxOutputBytes`. `maxDocuments`, `maxAliases` and `maxDecisions` are removed; requests that still send them fail with `INVALID_REQUEST`.
- Limits may now be raised above the defaults. `INVALID_LIMITS` is gone, and the CLI no longer clamps configured limits to the defaults.
- Decisions, mapping requests and duplicate requests are no longer truncated at 10,000. Credential choices stay bounded by the report byte budget.
- Removed the file-count caps: 100 sources in the CLI, the web app and batch envelopes, and the 10,000-path directory traversal cap. Batch envelopes no longer cap combined input at 20 MiB; each request keeps its own limits.
- `--timeout-ms` and the browser worker's `timeoutMs` accept any positive value instead of at most 30 seconds.
- `compare` reports every change; the 10,000-change cap and its `truncated` field are gone.
- Config files are read up to the 128 MiB transport limit instead of 1 MiB.
- `octri-spec-parse` no longer rejects parser limits above the defaults; the calling service sets them.
- Vendors octri-parser 0.2.0 and octri-auditor 0.2.0. Scripts and CI call `cargo` directly; `scripts/cargo.mjs` and `scripts/toolchain.mjs` are removed.
- npm packages ship as `0.2.0`.

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
