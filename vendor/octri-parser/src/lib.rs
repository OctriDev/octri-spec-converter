//! OpenAPI parsing, validation and reference transformations without ambient I/O.
//!
//! Adapters call [`execute`] or its document-only projection [`execute_document`].
//! Source documents remain immutable, and
//! [`Graph`] preserves shared identity and cycles without unsafe code.

mod bundle;
mod defaults;
mod diagnostics;
mod graph;
mod inspect;
pub mod numbers;
mod output;
mod pointer;
mod retrieval;
mod schema_dialect;
mod schema_registry;
mod session;
mod source;
mod syntax;
mod types;
mod validate;
pub mod wire;

pub use graph::{Graph, GraphNode, NodeId};
pub use output::{serialize, serialize_yaml};
pub use schema_dialect::{SchemaDialect, SchemaRoot};
pub use session::{NoIo, ReadContext, ResolvedSource, Resolver};
pub use types::*;

/// Decode bounded JSON/YAML without requiring an OpenAPI version declaration.
/// This is the same strict syntax reader used by the OpenAPI operations.
pub fn parse_source(uri: &str, text: String, limits: &Limits) -> Result<SourceDocument> {
    limits.check()?;
    source::parse(uri, text, limits)
}

/// Decode RAML syntax, retaining `!include` declarations as strings plus a
/// pointer-indexed include table. No included resource is read or executed.
pub fn parse_raml_source(uri: &str, text: String, limits: &Limits) -> Result<SyntaxDocument> {
    limits.check()?;
    source::parse_raml(uri, text, limits)
}

/// Bundle explicitly resolved resources while retaining physical source
/// addresses. This checks syntax and references; use Analyze for schema validation.
pub fn bundle_sources(
    input: Input,
    options: Options,
    resolver: &mut dyn Resolver,
) -> Result<BundledSource> {
    let mut session = session::Session::new(input.uri, &options, resolver, true)?;
    session.initialize(input.content, input.generated, false)?;
    finish_source_bundle(session, &options)
}

/// Bundle JSON Schema roots embedded in foreign documents into an OpenAPI 3.1+
/// envelope. The envelope refers to the physical roots by URI and JSON Pointer.
/// Only declared roots and schema reference targets are interpreted as schemas;
/// other foreign fields, examples and RAML include declarations remain inert.
/// Source files must be supplied explicitly or through the caller's resolver.
pub fn bundle_schema_sources(
    input: Input,
    schema_roots: Vec<Address>,
    options: Options,
    resolver: &mut dyn Resolver,
) -> Result<BundledSource> {
    bundle_schema_registry_sources(input, schema_roots, Vec::new(), options, resolver)
}

/// Bundle selected foreign schemas with an available component registry.
/// Registry roots establish resource IDs and anchors, but only selected roots
/// and their transitive schema dependencies are resolved and emitted. Duplicate
/// or malformed resource identifiers remain errors in the supplied registry.
pub fn bundle_schema_registry_sources(
    input: Input,
    schema_roots: Vec<Address>,
    registry_roots: Vec<Address>,
    options: Options,
    resolver: &mut dyn Resolver,
) -> Result<BundledSource> {
    bundle_schema_registry_with_dialects(
        input,
        schema_roots.into_iter().map(SchemaRoot::from).collect(),
        registry_roots.into_iter().map(SchemaRoot::from).collect(),
        options,
        resolver,
    )
}

/// Bundle foreign schemas using their declared format's default dialect.
/// Unlike editing `$schema` into virtual files, hints preserve source hashes,
/// locations, and the containing document's reference scope.
pub fn bundle_schema_registry_with_dialects(
    input: Input,
    schema_roots: Vec<SchemaRoot>,
    registry_roots: Vec<SchemaRoot>,
    options: Options,
    resolver: &mut dyn Resolver,
) -> Result<BundledSource> {
    let mut session = session::Session::new(input.uri, &options, resolver, true)?;
    session.initialize(input.content, input.generated, false)?;
    if !session.dialect.modern_schema() {
        return Err(ParserError::new(
            "INVALID_SCHEMA_ENVELOPE",
            "Foreign schema bundles require an OpenAPI 3.1 or later envelope",
            Address {
                uri: session.root_uri.clone(),
                pointer: String::new(),
            },
        ));
    }
    session.schema_mode = true;
    // Parents must establish resource scope before nested roots are indexed.
    let mut available = schema_roots.clone();
    available.extend(registry_roots);
    available.sort_by(|left, right| {
        left.address
            .uri
            .cmp(&right.address.uri)
            .then(left.address.pointer.len().cmp(&right.address.pointer.len()))
            .then(left.address.pointer.cmp(&right.address.pointer))
    });
    for root in &available {
        session.set_schema_default(&root.address, root.dialect)?;
    }
    available.dedup_by(|left, right| left.address == right.address);
    for root in &available {
        session.add_schema_root(&root.address)?;
    }
    session.select_schema_roots(
        &available
            .into_iter()
            .map(|root| root.address)
            .collect::<Vec<_>>(),
        &schema_roots
            .into_iter()
            .map(|root| root.address)
            .collect::<Vec<_>>(),
    )?;
    finish_source_bundle(session, &options)
}

fn finish_source_bundle(
    mut session: session::Session<'_>,
    options: &Options,
) -> Result<BundledSource> {
    session.resolve_all()?;
    if session
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
    {
        return Err(ParserError {
            message: "Cannot bundle sources with reference errors".into(),
            diagnostics: session.diagnostics,
        });
    }
    let bundled = bundle::bundle_with_sources(&session)?;
    output::check_document(&bundled.document, &options.limits)?;
    Ok(BundledSource {
        document: bundled.document,
        origins: bundled.origins,
        sources: session.sources.into_values().collect(),
        diagnostics: session.diagnostics,
        references: session.references,
    })
}

/// The return value of a document operation, without source/inventory reports.
/// A graph preserves shared identity and cycles when dereferencing is requested.
/// Parse/bundle return `document`; dereference/validate return `graph` instead.
/// Use [`Graph::to_value`] when a graph must be exported as a JSON tree.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DocumentOutput {
    pub document: Option<serde_json::Value>,
    pub graph: Option<Graph>,
}

/// Execute parse, bundle, dereference or validate without allocating unused source
/// location maps. Failures retain the same diagnostics as [`execute`].
pub fn execute_document(request: Request, resolver: &mut dyn Resolver) -> Result<DocumentOutput> {
    if !matches!(
        request.operation,
        Operation::Parse | Operation::Bundle | Operation::Dereference | Operation::Validate
    ) {
        return Err(ParserError::new(
            "INVALID_OPERATION",
            "Use execute for report and inventory operations",
            Address {
                uri: request.input.uri,
                pointer: String::new(),
            },
        ));
    }
    let report = execute_inner(request, resolver, false)?;
    Ok(DocumentOutput {
        document: report.document,
        graph: report.graph,
    })
}

/// Execute one isolated parser operation. No mutable parser instance is shared
/// between calls. `analyze` reports failures as diagnostics; other operations
/// return `ParserError` on invalid input or unresolved references.
pub fn execute(request: Request, resolver: &mut dyn Resolver) -> Result<Report> {
    execute_inner(request, resolver, true)
}

fn execute_inner(request: Request, resolver: &mut dyn Resolver, detailed: bool) -> Result<Report> {
    let analyze = request.operation == Operation::Analyze;
    let mut report = Report {
        valid: false,
        dialect: None,
        document: None,
        diagnostics: Vec::new(),
        sources: Vec::new(),
        references: Vec::new(),
        inspection: None,
        registry: ReferenceIndex::default(),
        graph: None,
    };
    let mut session =
        match session::Session::new(request.input.uri, &request.options, resolver, detailed) {
            Ok(session) => session,
            Err(error) if analyze => {
                report.diagnostics = error.diagnostics;
                return Ok(report);
            }
            Err(error) => return Err(error),
        };
    let result = run(
        request.operation,
        request.input.content,
        request.input.generated,
        &mut session,
        &mut report,
        detailed,
    );
    if let Err(error) = result {
        session.push_error(error);
    }
    if !detailed
        && session
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    {
        session.restore_diagnostic_locations()?;
    }
    report.diagnostics = session.diagnostics.clone();
    if detailed {
        report.registry = session.reference_index();
        report.sources = session.sources.into_values().collect();
        report.references = session.references;
    }
    report.valid = !report
        .diagnostics
        .iter()
        .any(|d| d.severity == Severity::Error);
    if !report.valid && !analyze {
        return Err(ParserError {
            message: report
                .diagnostics
                .first()
                .map_or_else(|| "Parser operation failed".into(), |d| d.message.clone()),
            diagnostics: report.diagnostics,
        });
    }
    Ok(report)
}

fn run(
    operation: Operation,
    content: Option<String>,
    generated: bool,
    session: &mut session::Session<'_>,
    report: &mut Report,
    detailed: bool,
) -> Result<()> {
    let normalize = session.options.validate.mode == ValidationMode::Compatible
        && matches!(operation, Operation::Validate | Operation::Analyze);
    session.initialize(content, generated, normalize)?;
    report.dialect = Some(session.dialect);
    if matches!(
        operation,
        Operation::Parse | Operation::Resolve | Operation::Inspect | Operation::Analyze
    ) {
        report.document = session
            .value(&Address {
                uri: session.root_uri.clone(),
                pointer: String::new(),
            })
            .cloned();
    }
    if operation == Operation::Parse {
        if let Some(document) = &report.document {
            output::check_document(document, &session.options.limits)?;
        }
        return Ok(());
    }
    session.resolve_all()?;
    if matches!(
        operation,
        Operation::Inspect | Operation::Validate | Operation::Analyze
    ) {
        report.inspection = Some(inspect::inspect(session));
    }
    if matches!(operation, Operation::Validate | Operation::Analyze)
        && let Some(inspection) = &report.inspection
    {
        for diagnostic in validate::validate(session, inspection) {
            session.push_diagnostic(diagnostic);
        }
    }
    if session
        .diagnostics
        .iter()
        .any(|d| d.severity == Severity::Error)
    {
        return Ok(());
    }
    if operation == Operation::Bundle {
        report.document = Some(bundle::bundle(session)?);
    }
    if matches!(operation, Operation::Dereference | Operation::Validate) {
        let graph = graph::dereference(session)?;
        let exported = if detailed {
            graph.to_value(&session.options.limits).map(Some)
        } else {
            graph.check_output(&session.options.limits).map(|()| None)
        };
        match exported {
            Ok(value) => report.document = value,
            Err(error)
                if error
                    .diagnostics
                    .iter()
                    .any(|d| d.code == "CIRCULAR_SERIALIZATION") =>
            {
                report.document = None
            }
            Err(error) => return Err(error),
        }
        report.graph = Some(graph);
    }
    // Graph export already checks expanded nodes, depth and encoded bytes.
    if report.graph.is_none()
        && let Some(document) = &report.document
    {
        output::check_document(document, &session.options.limits)?;
    }
    Ok(())
}
