# Contributing

Contributions are welcome. Preserve the project license and third-party notices. Start with a small, reproducible conversion example and consult the support matrix before extending an adapter.

Start with README's architecture and verification commands. Conversion behavior belongs in Rust. A binding transports the versioned JSON contract and manages host memory; it must not reimplement parsing, reference resolution, inference, schema migration or review semantics.

Use a small named function for a single responsibility. Explain invariants and non-obvious tradeoffs in comments. Preserve source bytes and physical addresses, keep literal example data inert, and distinguish authored decisions from inferred behavior. Avoid catch-all success paths that hide unsupported input or incomplete evidence.

For an adapter change, add representative source fixtures and assertions about resulting HTTP behavior, provenance and losses. Include a negative case that could otherwise produce a plausible but incorrect specification. Schema transformations need instance-validation evidence when equivalence is claimed. Transport changes need complete result parity and meaningful consumer tests across affected languages.

Treat all input as data. Never execute source scripts, commands or handlers. Rust core and bindings cannot create filesystem or network authority from an input URI. CLI and HTTP capabilities must be explicit, bounded and independently reviewed. Never add customer specifications or secrets to fixtures, logs or error snapshots.

Keep one writer per file. Do not replace another contributor's edits. Update public types, documentation and every affected binding together. Refresh vendored sources deliberately, regenerate notices from the locked dependency graph, update snapshot hashes, then qualify the resulting artifacts. Hash verification records identity; it is not a substitute for review.

Pull requests should describe the concrete before/after behavior, tests run, compatibility impact and remaining limits. Write code, comments, commit messages and pull request descriptions with enough context to stand alone.
