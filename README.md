# Octri Spec Converter

An offline Rust engine for converting API descriptions and request collections into Swagger/OpenAPI, with source provenance, strict validation, readiness auditing and explicit review decisions.

Maintained by [OctriDev](https://github.com/octridev). Build from source or use platform-matched release artifacts. Licensed under [Apache-2.0](LICENSE); third-party components retain their own licenses.

## Npm packages

```sh
npm install @octri/spec-converter-wasm
```

Use `@octri/spec-converter-native` for Node.js with platform-matched native addons.
Native packages select a pinned optional binary dependency for macOS ARM64 or
Linux x64/ARM64 with glibc; other platforms can use WASM or build from source.
Keep optional dependencies enabled. Node.js requires version 22.19 or later.

The complete static browser product is `@octri/spec-converter-web`.

Package READMEs contain installation examples and hosting requirements. Npm
wrapper patch versions can differ from the engine version recorded in results.
Other language registries are separate distribution targets; source-build
instructions remain below.

## Build and run

Use the toolchain pinned in `rust-toolchain.toml` and Node 24. Native binding builds additionally require the language toolchains listed in [language bindings](bindings/README.md).

```sh
npm ci
node scripts/cargo.mjs build --release --locked -p octri-converter-cli
target/release/octri-spec-convert convert api.yaml --target 3.1.2 --out openapi.json
```

Run `target/release/octri-spec-convert --help` for explicit source roots, supplied resources, batch conversion, review, verified exports and atomic file writes.

```sh
npm run build:wasm
node web/serve.mjs
```

Open `http://127.0.0.1:4178/web/index.html`. The browser uses a dedicated worker. It neither uploads source documents nor fetches their references. Fetching a URL is a separate user action subject to browser or host access controls.

## Conversion contract

Inputs include Swagger/OpenAPI, Postman, RAML, Insomnia, HAR, cURL, AsyncAPI, GraphQL SDL/introspection, Protobuf/gRPC and WSDL. `formats` returns the authoritative input and target catalogue. Targets include 2.0, 3.0.0–3.0.4, 3.1.0–3.1.2 and 3.2.0–3.2.1 in JSON or YAML.

The supported formats and boundaries below apply to source builds and release artifacts. Review each report before relying on a migration; valid output alone does not establish lossless conversion.

Non-HTTP protocols require explicit HTTP mappings. An authored mapping describes a chosen HTTP contract; it does not implement a gateway. Conversion never executes collection scripts, shell commands, GraphQL resolvers or API operations.

Every report distinguishes preserved, transformed, inferred, omitted and blocked information. Inferences and consequential losses need individual acknowledgement. Acknowledgement cannot clear validation errors or unresolved mappings. Only a valid, fully reviewed result permits handoff. Swagger 2.0 receives strict validation but has no invented Spec Auditor readiness score.

Reports retain original source text, physical locations, settings and hashes. Report import, comparison and export recompute the original conversion instead of trusting a stored `complete` flag. Review artifacts can contain credentials present in source files; inspect them before sharing.

## Architecture

- `crates/octri-converter`: conversion adapters, schema transformations, provenance, review and output emitters.
- `vendor/octri-parser`: shared bounded syntax/reference/validation engine; no ambient I/O.
- `vendor/octri-auditor`: shared static readiness checks; no API execution.
- `crates/octri-converter-cli`: explicit filesystem capabilities, batch orchestration and disposable workers.
- `crates/octri-converter-ffi`, `-node`, `-wasm`: transport and owned memory boundaries.
- `bindings`: C, Python, Go, Java, Kotlin, PHP, Ruby, Swift and Dart consumers.
- `packages`: Node ESM/CommonJS and browser packages sharing the same Rust engine.
- `web`: static source, conversion and review workspace.

## Verification

```sh
node scripts/verify-vendor.mjs
node scripts/cargo.mjs fmt --all -- --check
node scripts/cargo.mjs clippy --workspace --all-targets --locked -- -D warnings
npm run check
npm run build:node
node scripts/test-json.mjs
node scripts/test-node.mjs
npm run test:corpus
npm run build:bindings
npm run test:bindings
npm run build:wasm
npm run test:wasm:node
npm run test:ui:controller
npm run test:host:contract
node scripts/test-browser.mjs
node web/browser-test.mjs
```

Browser tests require Playwright Chromium, Firefox and WebKit. Binding and
installed-package checks are described in [bindings/README.md](bindings/README.md).
See [CONTRIBUTING.md](CONTRIBUTING.md) and [SECURITY.md](SECURITY.md).

## Supported formats

| Input                            | Support and boundaries                                                                                                                                                                                                                                               |
| -------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Swagger 2.0; OpenAPI 3.0/3.1/3.2 | Operations, schemas, security, references and explicit version migration. Features without an older-version equivalent require review; version strings are never substituted for conversion.                                                                         |
| Postman 2.0/2.1/3.0 HTTP         | Folders, supplied variables/environments, authentication, bodies and observed responses. Scripts remain inert; inferred schemas require review. Conditional credentials need explicit selection. Non-HTTP items need their protocol specification and HTTP mappings. |
| RAML 0.8/1.0                     | Supplied includes/libraries, traits/resource types, examples, security and supported type facets. Draft 3 schemas and unsupported template/XML semantics remain blocked or require authored mappings.                                                                |
| Insomnia 4/5                     | Collection requests, groups, supplied environments, auth, parameters and bodies. Embedded specifications remain evidence; import them directly to preserve their full contracts.                                                                                     |
| HAR                              | Observed HTTP exchanges. Captures do not establish every accepted input, response or auth requirement; inferred contracts require review.                                                                                                                            |
| cURL                             | Supported request flags, bodies and supplied files. Shell expansion, substitutions and ambient file reads are rejected. An observed command does not define a complete API contract.                                                                                 |
| AsyncAPI 2/3                     | Channels/messages, supported schemas and explicit HTTP mappings. Custom schema vocabularies need matching adapters. Legacy targets require review where tuple/dependency/resource semantics have no equivalent.                                                      |
| GraphQL SDL/introspection        | Types and operations with explicit HTTP mappings. Custom scalar serialization, resolver behavior and subscription transport cannot be inferred from names.                                                                                                           |
| Protobuf/gRPC                    | Supplied imports, supported ProtoJSON types and explicit HTTP mappings. Annotations/custom options remain review evidence; automatic transcoding and binary descriptor-set input are unsupported.                                                                    |
| WSDL 1.1/2.0 and supported XSD   | Supported native HTTP bindings and explicitly mapped SOAP contracts. XML serializer semantics require review. XSD redefine/override and unsupported SOAP bindings remain explicit blockers.                                                                          |

Select an input format explicitly when pasted GraphQL or cURL cannot be detected.
Every unsupported construct remains an error, blocker or reviewed omission.
Non-HTTP mappings describe an authored HTTP contract; they do not implement a gateway.

## JavaScript

```js
import { Converter, text } from "@octri/spec-converter-native";

const report = await Converter.convert({
  input: text(sourceText, "memory://project/api.yaml"),
  config: { targetVersion: "3.2.1" },
  files: {},
});
if (report.handoffAllowed) console.log(report.output.content);
```

The WASM package uses the same API. Reports preserve source text, physical
addresses, decisions, configuration and checksums. Review decisions by ID;
acknowledgement cannot override validation errors. Importing an older engine's
report requires reconverting its original sources with the current engine.

## Exact numbers

Rust preserves JSON integers and decimals. JavaScript decoded reports use `bigint`
for unsafe integers; use exported `parseJson`/`stringifyJson` or raw JSON transport
for storage. PHP and Dart reject lossy decoded numbers with
`LOSSLESS_NUMBER_REQUIRED`; their raw JSON methods preserve the original tokens.
Original source and Rust-produced output remain authoritative.

## Credential selection

Postman conditional URL matching is not evaluated automatically. Each pending
request exposes `credentialSelectionRequests` with source-bound choices. Set
`config.credentialSelections[request.pointer]` to an explicit choice:

```js
const item = report.credentialSelectionRequests[0];
const choice = item.choices[0]; // Replace with the user's selected choice.
const selection = {
  sourceUri: item.authSource.uri,
  sourceSha256: item.sourceSha256,
  authPointer: item.authSource.pointer,
  credentialIndex: choice.index,
};
```

Reconvert with that configuration and review its decisions. Source hashes prevent
replaying a choice against changed credentials. Secret vaults are never read.

## Self hosting

The browser package is a static worker application. Serve JavaScript and WASM with
correct MIME types and a CSP permitting same-origin workers and WebAssembly.
URL fetching is an explicit action requiring CORS or a bounded host proxy.

Applications hosting native conversion must isolate untrusted jobs and enforce
CPU, memory, process and request limits. Source URIs grant no I/O capability;
references must be supplied explicitly. Never log source or credentials.
