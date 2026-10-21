# Language bindings

[Live demo](https://octri.dev/spec-converter) · Free to try in your browser, no account required.

Every binding calls the same Rust conversion engine. Bindings add host memory
ownership, asynchronous execution where appropriate, and JSON transport. They do
not implement independent conversion rules. Build instructions and runtime requirements apply to source builds and
platform-specific release artifacts.

| Consumer | Runtime or build requirement                 | Transport                          |
| -------- | -------------------------------------------- | ---------------------------------- |
| Rust     | Rust 1.95                                    | Typed `octri_converter` API        |
| C        | C compiler and platform shared library       | Opaque owned UTF-8 buffers         |
| Node.js  | Node 22.19 or later                          | Node-API, ESM and CommonJS         |
| Browser  | WebAssembly and dedicated workers            | Worker-backed asynchronous client  |
| Python   | Python 3.8 or later                          | `ctypes` and native shared library |
| Go       | Go 1.23 or later, CGO enabled                | C ABI                              |
| Java     | JVM 11 or later                              | JNI and Jackson                    |
| Kotlin   | JVM 11 or later                              | Java/JNI facade                    |
| PHP      | PHP 8.1 or later with FFI enabled            | C ABI                              |
| Ruby     | Ruby 3.1 or later                            | Fiddle and C ABI                   |
| Swift    | Swift 6; macOS 13 or supported Linux runtime | C module                           |
| Dart     | Dart 3.4 or later                            | `dart:ffi`                         |

Native libraries must match the host architecture and operating system. A
macOS arm64 package cannot qualify Linux or Windows. CI configuration declares
platform checks; use completed runs and platform-specific manifests when distributing.
Java and Kotlin need their bundled Jackson/runtime dependencies alongside the
binding JARs. Go and Swift need a C compiler/linker; PHP deployments must allow
FFI explicitly. These are local library requirements, not Octri services.
Building Python wheels additionally requires a Python runtime supported by
setuptools 77 or later. The installed binding retains its Python 3.8 runtime floor.

## Contract and ownership

The wire envelope uses `abiVersion: 1` and an `action`: `formats`, `detect`,
`convert`, `review`, `bundle`, `batch`, `compare`, or `export`. Unknown fields, actions and
ABI versions fail explicitly. Reports have a versioned schema and contain exact
source text, configuration, locations, decisions and output checksums. Export and
comparison verify reports by recomputing their conversions.

The C API receives a pointer plus byte length and returns a Rust-owned result.
Read its exposed bytes before calling the matching free function exactly once.
Never free Rust allocations with the host allocator. Native facades enforce this
ownership boundary. JSON envelopes are UTF-8 and capped at 128 MiB. Core input,
nesting, node and output limits apply independently.

Core and native bindings perform no filesystem or network reads from source
identifiers. Supply every referenced file in the request. CLI and optional HTTP
host provide separate, bounded capabilities; a URI itself grants no access.

See [exact-number handling](../README.md#exact-numbers) for exact decimals, unsafe integers and decoded
representations. Use raw JSON methods when the host cannot decode an exact
number. PHP and Dart return `LOSSLESS_NUMBER_REQUIRED` instead of silently
rounding unsafe values. Ordinary safe values keep their usual host types.

## Build and verify

Postman credential choices use the same source-bound JSON configuration in every
binding; see [credential selection](../README.md#credential-selection). No binding evaluates URL
matching rules independently of Rust.

```sh
node scripts/build-node.mjs
node scripts/build-wasm.mjs
node scripts/build-bindings.mjs
node scripts/test-node.mjs
node scripts/test-browser.mjs
node scripts/test-bindings.mjs
node scripts/notices.mjs
node scripts/pack-bindings.mjs
node scripts/test-binding-packages.mjs
node scripts/test-package.mjs
```

`test-bindings` compares complete raw results with the Rust CLI across all eight
native language facades. `test-binding-packages` installs or extracts release
artifacts outside the checkout and tests relocated libraries and fresh consumers.
It includes Rust as well as Python, Ruby, PHP, Java, Kotlin, Go, Dart and Swift.
Node/browser package tests separately check installed entry points and TypeScript
declarations. None of these commands publishes packages.

Release artifacts live under `artifacts/release/<platform>-<architecture>` with
SHA-256 manifests, native libraries, source packages and legal notices. Keep
the manifest with the exact artifacts that passed qualification. A successful
source checkout build does not substitute for installed-consumer tests.
