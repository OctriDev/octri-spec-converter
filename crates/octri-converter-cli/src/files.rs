//! Explicit, bounded file admission and atomic output publication.
use crate::{Error, Result, args::Settings};
use octri_converter::{Config, ConversionRequest};
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

pub(crate) fn json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    octri_parser::wire::decode_json("", &read(path, MAX_REPORT_BYTES)?)
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
            .map(json::<Config>)
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

    /// Read one source within the configured document and combined byte limits.
    fn load(&mut self, path: &Path) -> Result<String> {
        let limits = &self.config.limits;
        let content = read(path, limits.max_document_bytes)?;
        let next = self.bytes.saturating_add(content.len());
        if next > limits.max_total_bytes {
            return Err(Error::Message(
                "Sources exceed the combined byte limit".into(),
            ));
        }
        self.bytes = next;
        Ok(content)
    }

    fn admit(&mut self, uri: String, path: &Path) -> Result<()> {
        if let Some(previous) = self.dependencies.get(&uri) {
            if previous != &read(path, self.config.limits.max_document_bytes)? {
                return Err(Error::Message(format!("Conflicting source URI: {uri}")));
            }
            return Ok(());
        }
        let content = self.load(path)?;
        self.protected.insert(path.to_owned());
        self.dependencies.insert(uri, content);
        Ok(())
    }

    pub fn request(&mut self, path: &Path) -> Result<ConversionRequest> {
        let path = self.fenced(path)?;
        let uri = self.uri(&path)?;
        let mut files = self.dependencies.clone();
        let content = match files.remove(&uri).or_else(|| self.roots.get(&uri).cloned()) {
            Some(content) => content,
            None => {
                let content = self.load(&path)?;
                self.roots.insert(uri.clone(), content.clone());
                content
            }
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
