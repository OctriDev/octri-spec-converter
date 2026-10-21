//! Rule metadata is public product behavior: IDs and weights are migration contracts.

use crate::Category;
use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Rule {
    pub id: &'static str,
    pub title: &'static str,
    pub weight: f64,
    pub surface: &'static str,
    pub categories: &'static [Category],
    pub why: &'static str,
    pub remediation: &'static str,
}

const DOCS_SDK: &[Category] = &[Category::Docs, Category::Sdk];
const SDK: &[Category] = &[Category::Sdk];
const MCP: &[Category] = &[Category::Mcp];
const ALL: &[Category] = &[Category::Docs, Category::Sdk, Category::Mcp];
const INTEGRITY: &[Category] = &[
    Category::Integrity,
    Category::Docs,
    Category::Sdk,
    Category::Mcp,
];

macro_rules! rule {
    ($id:literal, $title:literal, $weight:literal, $surface:literal, $categories:ident, $why:literal, $remediation:literal) => {
        Rule {
            id: $id,
            title: $title,
            weight: $weight,
            surface: $surface,
            categories: $categories,
            why: $why,
            remediation: $remediation,
        }
    };
}

pub static RULES: &[Rule] = &[
    rule!(
        "request-schema",
        "Request bodies declare a schema",
        0.0,
        "operations",
        ALL,
        "A request without a schema has no dependable generated argument contract.",
        "Author the actual request schema or reference an existing component; the auditor never invents payload structure."
    ),
    rule!(
        "spec-integrity",
        "Fix invalid specification structure",
        0.0,
        "document",
        INTEGRITY,
        "These errors make generated coverage incomplete or ambiguous.",
        "Correct the source error at the reported location, then audit again."
    ),
    rule!(
        "response-schema",
        "Successful responses declare a schema",
        2.00,
        "operations",
        DOCS_SDK,
        "Without a response schema every method returns `unknown`/`any`, so callers get no types and no autocompletion.",
        "Declare the actual response schema; 204 and 205 responses do not need a body."
    ),
    rule!(
        "request-body-documented",
        "Request bodies are documented",
        1.00,
        "operations",
        DOCS_SDK,
        "Body field descriptions become the doc comments on generated parameters; without them the SDK ships undocumented arguments.",
        "Describe the request body or its schema properties using the API contract."
    ),
    rule!(
        "operation-id-unique",
        "Generated public names do not collide",
        2.00,
        "operations",
        SDK,
        "When authored identifiers collapse to the same shared public name, the generator must silently rename one method; callers otherwise cannot predict the SDK surface from the spec.",
        "Review distinct operation identifiers and namespaces against the selected naming profile."
    ),
    rule!(
        "operation-id-present",
        "Every operation declares an operationId",
        1.00,
        "operations",
        SDK,
        "Without one the generator synthesizes a name from the path, which changes whenever the path does. That is a silent breaking change for callers.",
        "Choose an explicit stable operationId, for example listPets. This changes generated public names."
    ),
    rule!(
        "operation-id-naming",
        "operationIds read as method names",
        1.00,
        "operations",
        SDK,
        "The id becomes the method name. `ActionsActionsGet` generates `client.actions.actionsActionsGet()`; `getActions` generates `client.actions.get()`.",
        "Use a meaningful camelCase identifier; review generated API changes before renaming."
    ),
    rule!(
        "duplicate-paths",
        "No duplicate or equivalent paths",
        1.00,
        "paths",
        ALL,
        "Paths differing only in parameter NAME are the same route to a server; the generated SDK ends up with two methods that call the same endpoint.",
        "Review equivalent templates such as /pets/{id} and /pets/{name}; consolidate the declared contract."
    ),
    rule!(
        "shared-schemas-referenced",
        "Shared models live in components and are $ref'd",
        2.00,
        "operations",
        SDK,
        "Inline object schemas become anonymous per-operation types. The same model inlined in 20 operations generates 20 near-identical types instead of one.",
        "Move deliberately shared models to components.schemas and reference them without changing requiredness."
    ),
    rule!(
        "operation-documented",
        "Operations carry a summary or description",
        1.00,
        "operations",
        ALL,
        "Operation prose becomes the method's doc comment, the text an IDE shows on hover.",
        "Describe the operation's actual behavior. Descriptions are authored content, never automatic fixes."
    ),
    rule!(
        "error-responses",
        "Failure responses are declared",
        1.00,
        "operations",
        DOCS_SDK,
        "Declared error schemas become typed errors. Without them every failure surfaces as an untyped generic error.",
        "Declare actual 4xx/5xx or default responses and their payloads."
    ),
    rule!(
        "operation-tagged",
        "Operations are tagged",
        1.00,
        "operations",
        DOCS_SDK,
        "Tags drive namespacing. Untagged operations fall back to a path segment, producing lopsided or `misc` namespaces.",
        "Choose a meaningful group. Tags can change generated names and require review."
    ),
    rule!(
        "path-parameters-declared",
        "Every path placeholder has a parameter",
        1.50,
        "operations",
        ALL,
        "An undeclared `{id}` placeholder cannot become a typed method argument, so the generated request path is incomplete or uncompilable.",
        "Declare the placeholder with in: path, required: true and the actual wire type."
    ),
    rule!(
        "parameters-documented",
        "Parameters explain their purpose",
        0.75,
        "operations",
        DOCS_SDK,
        "Parameter descriptions become IDE hints on generated method arguments; bare names force SDK users back to the API reference.",
        "Describe purpose, units and restrictions from the actual API contract."
    ),
    rule!(
        "server-url",
        "A server URL is declared",
        0.50,
        "document",
        ALL,
        "Without a server URL the generated client must ship with an empty or placeholder base URL, adding setup work and easy production misconfiguration.",
        "Declare the actual server URL; relative URLs are valid when deployment supplies the origin."
    ),
    rule!(
        "security-declared",
        "Authentication is described",
        1.00,
        "document",
        ALL,
        "Security schemes generate the SDK's auth configuration. A spec without them produces a client with no way to authenticate.",
        "Define authentication schemes in components.securitySchemes. For an intentionally public API, disable this readiness rule in policy; security: [] declares anonymous access but does not define an SDK authentication scheme."
    ),
    rule!(
        "schema-constraint-applicability",
        "Schema constraints apply to their declared type",
        0.0,
        "schemas",
        SDK,
        "Inapplicable constraints cannot be enforced consistently by generated SDK validators.",
        "Review the declared type and the intended constraint; absent type can be intentional JSON Schema behavior."
    ),
    rule!(
        "schema-enum-domain",
        "Enum members match the declared wire type",
        0.0,
        "schemas",
        SDK,
        "Impossible enum members cannot be represented without changing their wire value.",
        "Align enum values with the declared type; do not coerce wire values."
    ),
    rule!(
        "schema-example-type",
        "Examples match their declared type",
        0.0,
        "schemas",
        ALL,
        "Contradictory examples generate misleading documentation and tests.",
        "Supply a real example satisfying the declared schema."
    ),
    rule!(
        "package-version-source",
        "API metadata provides a package-compatible version",
        0.0,
        "document",
        SDK,
        "API versions may need normalization before package publication.",
        "Keep the API version intact; configure a separate semantic package version when required."
    ),
    rule!(
        "schema-constraints",
        "Schema constraints are consistent",
        0.0,
        "schemas",
        ALL,
        "Contradictory constraints describe values no caller can send or receive.",
        "Reconcile minimum/maximum, lengths, cardinalities and enum/const intersections."
    ),
    rule!(
        "schema-example-valid",
        "Authored examples satisfy their schema",
        0.0,
        "schemas",
        ALL,
        "Examples violating constraints mislead consumers even when their basic type matches.",
        "Review the failing schema constraint and replace the authored example."
    ),
    rule!(
        "schema-example-unverified",
        "Example validation coverage is explicit",
        0.0,
        "schemas",
        ALL,
        "An unsupported schema must not produce a false example-validation pass.",
        "Review the reported vocabulary or reference limitation; run an appropriate validator before relying on the example."
    ),
    rule!(
        "schema-composition",
        "Composition limitations are visible",
        0.0,
        "schemas",
        SDK,
        "Composition and discriminator behavior varies across generator targets.",
        "Review alternatives and discriminator mappings with the selected generator; static analysis does not prove generated runtime behavior."
    ),
    rule!(
        "schema-dialect",
        "Schema keywords match the declared dialect",
        0.0,
        "schemas",
        ALL,
        "Nullable and type keywords have different meanings across OpenAPI dialects.",
        "Use nullable in 3.0 and JSON Schema null types in 3.1/3.2; never relabel the version alone."
    ),
    rule!(
        "sdk-target-compatibility",
        "Selected SDK target limitations are visible",
        0.0,
        "schemas",
        SDK,
        "Dynamic schema constructs and binary/numeric domains differ across native target languages.",
        "Review the source construct against the named target profile and execute that generator before publication."
    ),
    rule!(
        "mcp-tool-name",
        "MCP tool names are usable and unique",
        0.0,
        "operations",
        MCP,
        "MCP clients need stable, distinct tool identifiers.",
        "Review identifiers matching [A-Za-z0-9_.-]{1,128}; source renames need approval."
    ),
    rule!(
        "mcp-input-schema",
        "MCP inputs have a representable object schema",
        0.0,
        "operations",
        MCP,
        "Tool arguments must retain requiredness and supported schema semantics.",
        "Review non-object request bodies, parameter collisions and incompatible argument composition."
    ),
    rule!(
        "operation-examples",
        "Operations provide request or response examples",
        0.0,
        "operations",
        DOCS_SDK,
        "Concrete examples help developers construct and recognize valid messages.",
        "Supply real, non-sensitive examples and validate them against their schemas."
    ),
    rule!(
        "path-parameter-required",
        "Path parameters are explicitly required",
        0.0,
        "operations",
        ALL,
        "OpenAPI requires path parameters to set required: true.",
        "The deterministic patch adds required: true to the existing path parameter; review before applying."
    ),
    rule!(
        "project-size",
        "Consider splitting this into several SDKs",
        0.0,
        "document",
        SDK,
        "Large generated packages increase install and IDE costs.",
        "Consider separate packages for deliberately independent API groups."
    ),
];

pub fn find(id: &str) -> Option<&'static Rule> {
    RULES.iter().find(|rule| rule.id == id)
}
