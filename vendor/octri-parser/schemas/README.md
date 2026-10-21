# OpenAPI validation schemas

These schema resources come directly from the OpenAPI Initiative publication site,
under Apache-2.0. The Rust engine compiles the checked-in files offline. No remote
schema retrieval or third-party OpenAPI parser is used at runtime.

| Resource | Official revision |
| --- | --- |
| `openapi-2.0.json` | [2017-08-27](https://spec.openapis.org/oas/2.0/schema/2017-08-27) |
| `openapi-3.0.json` | [2024-10-18](https://spec.openapis.org/oas/3.0/schema/2024-10-18) |
| `openapi-3.1.json` | [2026-08-03](https://spec.openapis.org/oas/3.1/schema/2026-08-03) |
| `openapi-3.2.json` | [2026-08-30](https://spec.openapis.org/oas/3.2/schema/2026-08-30) |

`OAI-LICENSE` retains the upstream license. `sources.json` records source URLs,
schema IDs and SHA-256 hashes. Files preserve upstream JSON values; a trailing
newline is normalized for source control. Run `node scripts/verify-schemas.mjs`
to verify checked-in resources before building.

Updates require reviewing upstream changes and running the dialect, semantic,
reference and cross-language regression suites. Never substitute mutable network
content during a customer request.
