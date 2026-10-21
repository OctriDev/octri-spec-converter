//! CLI syntax is separate from conversion and host capabilities.
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "octri-spec-convert",
    version,
    about = "Offline specification conversion with explicit fidelity review"
)]
pub(crate) struct Cli {
    /// Kill the conversion worker after this many milliseconds (maximum 30000).
    #[arg(long, global = true, default_value_t = 30_000, value_parser = clap::value_parser!(u64).range(1..=30_000))]
    pub timeout_ms: u64,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Read one versioned JSON envelope from stdin; also the no-argument mode.
    Execute,
    /// Convert one source. Review-required output remains a draft.
    Convert(Convert),
    /// Convert and bundle references using explicitly supplied files only.
    Bundle(Convert),
    /// Detect a source format without converting it.
    Detect(Source),
    /// List supported formats and exact target versions.
    Formats,
    /// Reconvert a saved report with specific decision acknowledgements.
    Review(Review),
    /// Convert files, directories or globs; continue after individual failures.
    Batch(Batch),
    /// Verify two reports and compare their converted documents.
    Compare {
        before: PathBuf,
        after: PathBuf,
        #[command(flatten)]
        output: Destination,
    },
    /// Verify and export a saved report, manifest, HTML or original-file bundle.
    Export {
        report: PathBuf,
        #[arg(long, value_enum, default_value_t = ExportFormat::Json)]
        format: ExportFormat,
        /// Artifact export writes a new directory. Other exports write a file.
        #[command(flatten)]
        output: Destination,
    },
}

#[derive(Debug, Args)]
pub(crate) struct Source {
    pub input: PathBuf,
    #[command(flatten)]
    pub settings: Settings,
}

#[derive(Debug, Args)]
pub(crate) struct Settings {
    /// Resolve source paths under this directory. Symlinks cannot escape it.
    #[arg(long, default_value = ".")]
    pub root_dir: PathBuf,
    /// Additional source file, directory or glob, relative to root-dir; repeatable.
    #[arg(long = "file")]
    pub files: Vec<String>,
    /// Bind a logical source URI to a file under root-dir: URI=PATH.
    #[arg(long = "resource")]
    pub resources: Vec<String>,
    /// JSON configuration, using the same schema as every binding.
    #[arg(long)]
    pub config: Option<PathBuf>,
    /// Explicit input format; omitted means automatic detection.
    #[arg(long, value_parser = ["auto", "openapi", "swagger", "postman", "raml", "insomnia", "har", "curl", "asyncapi", "graphql", "protobuf", "wsdl"])]
    pub input_format: Option<String>,
    /// Exact published Swagger/OpenAPI version.
    #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(octri_converter::TARGET_VERSIONS.iter().copied()))]
    pub target: Option<String>,
    #[arg(long, value_parser = ["json", "yaml"])]
    pub format: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct Convert {
    #[command(flatten)]
    pub source: Source,
    #[command(flatten)]
    pub output: Destination,
    /// Save the full reproducible report, including original source bytes.
    #[arg(long)]
    pub report: Option<PathBuf>,
    /// Write the full report to stdout instead of the converted document.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct Destination {
    #[arg(short, long)]
    pub out: Option<PathBuf>,
    /// Atomically replace an existing output. Source files are always protected.
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub(crate) struct Review {
    pub report: PathBuf,
    /// Apply a replacement configuration before reconversion.
    #[arg(long)]
    pub config: Option<PathBuf>,
    /// Acknowledge one review decision ID; repeatable. Never clears blockers.
    #[arg(long)]
    pub accept: Vec<String>,
    #[command(flatten)]
    pub output: Destination,
}

#[derive(Debug, Args)]
pub(crate) struct Batch {
    /// Files, directories or globs relative to root-dir. Quote globs in the shell.
    #[arg(required = true)]
    pub inputs: Vec<String>,
    #[command(flatten)]
    pub settings: Settings,
    /// New directory for converted documents, reports and aggregate.json.
    #[arg(long)]
    pub out_dir: PathBuf,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum ExportFormat {
    Json,
    Html,
    Manifest,
    Artifact,
}

impl ExportFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Html => "html",
            Self::Manifest => "manifest",
            Self::Artifact => "artifact",
        }
    }
}
