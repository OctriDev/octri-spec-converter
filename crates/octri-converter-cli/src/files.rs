//! Explicit, bounded file admission and atomic output publication.
use crate::{Error, Result, args::Settings};
use octri_converter::{Config, ConversionRequest, Limits};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

pub(crate) const MAX_REPORT_BYTES: usize = octri_converter::MAX_ENVELOPE_BYTES;

pub(crate) fn read(path: &Path, limit: usize) -> Result<String> {
    let file = File::open(path)?;
    if !file.metadata()?.is_file() || file.metadata()?.len() > limit as u64 {
        return Err(Error::Message(format!(
            "Not a regular file within the {limit}-byte limit: {}",
            path.display()
        )));
    }
    let mut content = String::new();
    file.take(limit as u64 + 1).read_to_string(&mut content)?;
    if content.len() > limit {
        return Err(Error::Message(format!(
            "File grew beyond the {limit}-byte limit: {}",
            path.display()
        )));
    }
    Ok(content)
}

pub(crate) fn json(path: &Path) -> Result<Value> {
    octri_parser::wire::decode_json("", &read(path, MAX_REPORT_BYTES)?)
        .map_err(|error| Error::Message(error.to_string()))
}

pub(crate) fn config(path: &Path) -> Result<Config> {
    octri_parser::wire::decode_json("", &read(path, 1024 * 1024)?)
        .map_err(|error| Error::Message(error.to_string()))
}

#[derive(Debug)]
pub(crate) struct Inputs {
    pub root: PathBuf,
    pub config: Config,
    pub protected: BTreeSet<PathBuf>,
    dependencies: BTreeMap<String, String>,
    roots: BTreeMap<String, String>,
    bytes: usize,
}

impl Inputs {
    pub fn new(settings: &Settings) -> Result<Self> {
        let root = settings.root_dir.canonicalize()?;
        if !root.is_dir() {
            return Err(Error::Message("root-dir must be a directory".into()));
        }
        let mut config = settings
            .config
            .as_deref()
            .map(config)
            .transpose()?
            .unwrap_or_default();
        if let Some(format) = &settings.input_format {
            config.input_format = serde_json::from_value(Value::String(format.clone()))?;
        }
        if let Some(target) = &settings.target {
            config.target_version = Some(target.clone());
        }
        if let Some(format) = &settings.format {
            config.output_format = serde_json::from_value(Value::String(format.clone()))?;
        }
        let mut inputs = Self {
            root,
            config,
            protected: BTreeSet::new(),
            dependencies: BTreeMap::new(),
            roots: BTreeMap::new(),
            bytes: 0,
        };
        if let Some(path) = &settings.config {
            inputs.protected.insert(path.canonicalize()?);
        }
        for path in inputs.expand(&settings.files, false)? {
            let uri = inputs.uri(&path)?;
            inputs.admit(uri, &path)?;
        }
        for resource in &settings.resources {
            let (uri, path) = resource
                .split_once('=')
                .ok_or_else(|| Error::Message("resource requires URI=PATH".into()))?;
            let url = url::Url::parse(uri).map_err(|error| Error::Message(error.to_string()))?;
            if !matches!(url.scheme(), "memory" | "file" | "http" | "https")
                || url.fragment().is_some()
                || !url.username().is_empty()
                || url.password().is_some()
            {
                return Err(Error::Message(
                    "Resource URI requires a supported scheme and no credentials or fragment"
                        .into(),
                ));
            }
            let path = inputs.fenced(Path::new(path))?;
            inputs.admit(url.to_string(), &path)?;
        }
        Ok(inputs)
    }

    fn admit(&mut self, uri: String, path: &Path) -> Result<()> {
        let limits = &self.config.limits;
        let content = read(
            path,
            limits
                .max_document_bytes
                .min(Limits::default().max_document_bytes),
        )?;
        if let Some(previous) = self.dependencies.get(&uri) {
            if previous != &content {
                return Err(Error::Message(format!("Conflicting source URI: {uri}")));
            }
            return Ok(());
        }
        let next = self.bytes.saturating_add(content.len());
        if next
            > limits
                .max_total_bytes
                .min(Limits::default().max_total_bytes)
            || self.dependencies.len() >= limits.max_documents.min(Limits::default().max_documents)
        {
            return Err(Error::Message(
                "Supplied sources exceed document or combined byte limit".into(),
            ));
        }
        self.bytes = next;
        self.protected.insert(path.to_owned());
        self.dependencies.insert(uri, content);
        Ok(())
    }

    pub fn request(&mut self, path: &Path) -> Result<ConversionRequest> {
        let path = self.fenced(path)?;
        let uri = self.uri(&path)?;
        let mut files = self.dependencies.clone();
        let content = if let Some(content) =
            files.remove(&uri).or_else(|| self.roots.get(&uri).cloned())
        {
            content
        } else {
            let content = read(
                &path,
                self.config
                    .limits
                    .max_document_bytes
                    .min(Limits::default().max_document_bytes),
            )?;
            let next = self.bytes.saturating_add(content.len());
            if next
                > self
                    .config
                    .limits
                    .max_total_bytes
                    .min(Limits::default().max_total_bytes)
                || self.dependencies.len() + self.roots.len()
                    >= self
                        .config
                        .limits
                        .max_documents
                        .min(Limits::default().max_documents)
            {
                return Err(Error::Message(
                    "Batch sources exceed document or combined byte limit; split the batch".into(),
                ));
            }
            self.bytes = next;
            self.roots.insert(uri.clone(), content.clone());
            content
        };
        self.protected.insert(path);
        Ok(ConversionRequest {
            input: octri_parser::Input {
                uri,
                content: Some(content),
                generated: false,
            },
            files,
            config: self.config.clone(),
        })
    }

    pub fn fenced(&self, path: &Path) -> Result<PathBuf> {
        let path = if path.is_absolute() {
            path.to_owned()
        } else {
            self.root.join(path)
        }
        .canonicalize()?;
        if !path.starts_with(&self.root) {
            return Err(Error::Message(format!(
                "Source escapes root-dir: {}",
                path.display()
            )));
        }
        Ok(path)
    }

    pub fn uri(&self, path: &Path) -> Result<String> {
        let relative = path
            .strip_prefix(&self.root)
            .map_err(|_| Error::Message("Source outside root-dir".into()))?;
        let mut uri = url::Url::parse("memory://project/")
            .map_err(|error| Error::Message(error.to_string()))?;
        {
            let mut segments = uri
                .path_segments_mut()
                .map_err(|_| Error::Message("Invalid source URI base".into()))?;
            segments.pop_if_empty();
            for segment in relative.components() {
                let Component::Normal(segment) = segment else {
                    return Err(Error::Message("Invalid source path".into()));
                };
                segments.push(
                    segment
                        .to_str()
                        .ok_or_else(|| Error::Message("Source filenames must be UTF-8".into()))?,
                );
            }
        }
        Ok(uri.to_string())
    }

    pub fn expand(&self, patterns: &[String], roots_only: bool) -> Result<Vec<PathBuf>> {
        let mut files = BTreeSet::new();
        let mut examined = 0usize;
        for pattern in patterns {
            let supplied = Path::new(pattern);
            if supplied
                .components()
                .any(|part| matches!(part, Component::ParentDir))
            {
                return Err(Error::Message(
                    "Source pattern escapes root-dir through parent traversal".into(),
                ));
            }
            let relative = if supplied.is_absolute() {
                supplied
                    .strip_prefix(&self.root)
                    .map_err(|_| Error::Message("Source pattern escapes root-dir".into()))?
            } else {
                supplied
            };
            let literal = self.root.join(relative);
            let (start, matcher) = if literal.exists() {
                (literal, None)
            } else {
                let text = relative
                    .to_str()
                    .ok_or_else(|| Error::Message("Source pattern must be UTF-8".into()))?;
                let matcher =
                    glob::Pattern::new(text).map_err(|error| Error::Message(error.to_string()))?;
                let mut prefix = self.root.clone();
                for component in relative.components() {
                    let value = component
                        .as_os_str()
                        .to_str()
                        .ok_or_else(|| Error::Message("Source pattern must be UTF-8".into()))?;
                    if value.contains(['*', '?', '[']) {
                        break;
                    }
                    prefix.push(component);
                }
                (prefix, Some(matcher))
            };
            let filter_extensions = roots_only && matcher.is_none() && start.is_dir();
            let mut pending = vec![start];
            let mut visited = BTreeSet::new();
            let mut matches = 0usize;
            while let Some(path) = pending.pop() {
                examined += 1;
                if examined > 10_000 {
                    return Err(Error::Message(
                        "Source traversal exceeds 10000 paths".into(),
                    ));
                }
                let canonical = self.fenced(&path)?;
                if canonical.is_dir() {
                    // Glob libraries commonly follow directory symlinks before
                    // yielding results. Traverse ourselves, checking each step.
                    if fs::symlink_metadata(&path)?.file_type().is_symlink() {
                        return Err(Error::Message("Directory symlinks are not traversed; supply their target under root-dir explicitly".into()));
                    }
                    if !visited.insert(canonical) {
                        continue;
                    }
                    for entry in fs::read_dir(path)? {
                        pending.push(entry?.path());
                        if pending.len() > 10_000 {
                            return Err(Error::Message(
                                "Source traversal exceeds 10000 paths".into(),
                            ));
                        }
                    }
                } else if canonical.is_file() {
                    let selected = matcher.as_ref().is_none_or(|matcher| {
                        path.strip_prefix(&self.root).is_ok_and(|relative| {
                            matcher.matches_path_with(
                                relative,
                                glob::MatchOptions {
                                    case_sensitive: true,
                                    require_literal_separator: true,
                                    require_literal_leading_dot: true,
                                },
                            )
                        })
                    });
                    if !selected || (filter_extensions && !source_extension(&path)) {
                        continue;
                    }
                    matches += 1;
                    files.insert(canonical);
                    if files.len() > 100 {
                        return Err(Error::Message("Select at most 100 source files".into()));
                    }
                } else {
                    return Err(Error::Message(
                        "Only regular source files and directories are accepted".into(),
                    ));
                }
            }
            if matches == 0 {
                return Err(Error::Message(format!("No source matches: {pattern}")));
            }
        }
        Ok(files.into_iter().collect())
    }
}

fn source_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "json"
                    | "yaml"
                    | "yml"
                    | "raml"
                    | "graphql"
                    | "gql"
                    | "proto"
                    | "wsdl"
                    | "har"
                    | "curl"
                    | "http"
            )
        })
}

pub(crate) fn write(
    path: &Path,
    content: &[u8],
    force: bool,
    protected: &BTreeSet<PathBuf>,
) -> Result<()> {
    if content.len() > MAX_REPORT_BYTES {
        return Err(Error::Message("Output exceeds 128 MiB".into()));
    }
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = parent.canonicalize()?;
    let name = path
        .file_name()
        .ok_or_else(|| Error::Message("Output must name a file".into()))?;
    let destination = parent.join(name);
    let identity = if destination.exists() {
        destination.canonicalize()?
    } else {
        destination.clone()
    };
    if protected.contains(&identity) || protected.contains(&destination) {
        return Err(Error::Message(
            "Output would overwrite an input or configuration file".into(),
        ));
    }
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(content)?;
    temporary.as_file().sync_all()?;
    if force {
        temporary
            .persist(destination)
            .map_err(|error| Error::Io(error.error))?;
    } else {
        temporary
            .persist_noclobber(destination)
            .map_err(|error| Error::Io(error.error))?;
    }
    Ok(())
}
