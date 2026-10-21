//! Deterministic fidelity decisions. Review acknowledgements cannot clear blockers.

use crate::source::{Sources, hash};
use crate::{
    Classification, Config, CredentialSelectionRequest, Decision, DuplicateRequest, MappingRequest,
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug)]
pub(crate) struct Trace {
    pub decisions: Vec<Decision>,
    pub mapping_requests: BTreeMap<String, MappingRequest>,
    pub duplicate_requests: BTreeMap<String, DuplicateRequest>,
    pub credential_requests: BTreeMap<String, CredentialSelectionRequest>,
    credential_units: usize,
    credential_bytes: usize,
    max_credential_bytes: usize,
    source_hashes: BTreeMap<String, String>,
    keys: BTreeSet<String>,
    accepted: BTreeSet<String>,
    max: usize,
    pub truncated: bool,
    context_hash: String,
}

impl Trace {
    pub fn new(config: &Config, sources: &Sources) -> Self {
        let mut semantic_config = config.clone();
        semantic_config.accepted_decisions.clear();
        let files = sources.manifest();
        let source_hashes = files
            .iter()
            .map(|file| (file.uri.clone(), file.sha256.clone()))
            .collect();
        let mut context = serde_json::json!({"config":semantic_config,"files":files});
        octri_parser::numbers::canonicalize(&mut context);
        let mut context = context.to_string();
        context.push_str(crate::ENGINE_VERSION);
        Self {
            decisions: Vec::new(),
            mapping_requests: BTreeMap::new(),
            duplicate_requests: BTreeMap::new(),
            credential_requests: BTreeMap::new(),
            credential_units: 0,
            credential_bytes: 2,
            max_credential_bytes: config
                .limits
                .max_output_bytes
                .min(crate::MAX_ENVELOPE_BYTES / 4),
            source_hashes,
            keys: BTreeSet::new(),
            accepted: config.accepted_decisions.clone(),
            max: config.limits.max_decisions,
            truncated: false,
            context_hash: hash(context.as_bytes()),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record(
        &mut self,
        sources: &Sources,
        uri: &str,
        pointer: &str,
        output: &[String],
        code: &str,
        class: Classification,
        message: impl Into<String>,
        evidence: Option<&Value>,
    ) {
        let serialized = evidence.map_or_else(String::new, |value| {
            let mut value = value.clone();
            octri_parser::numbers::canonicalize(&mut value);
            value.to_string()
        });
        let id = hash(
            format!(
                "{}\n{uri}\n{pointer}\n{code}\n{class:?}\n{}\n{serialized}",
                self.context_hash,
                output.join("\n")
            )
            .as_bytes(),
        );
        if self.keys.contains(&id) {
            return;
        }
        if self.decisions.len() >= self.max {
            self.truncated = true;
            return;
        }
        self.keys.insert(id.clone());
        let review_required = matches!(
            class,
            Classification::Inferred | Classification::Omitted | Classification::Blocked
        );
        let reviewed = self.accepted.contains(&id) && class != Classification::Blocked;
        self.decisions.push(Decision {
            id,
            code: code.into(),
            classification: class,
            source: sources.location(uri, pointer),
            output_pointers: output.to_vec(),
            message: message.into(),
            remediation: None,
            review_required,
            reviewed,
        });
    }

    pub fn blocked(&self) -> bool {
        self.truncated
            || self
                .decisions
                .iter()
                .any(|item| item.classification == Classification::Blocked)
    }

    pub fn describe_mapping(&mut self, request: MappingRequest) {
        if self.mapping_requests.len() >= self.max
            && !self.mapping_requests.contains_key(&request.key)
        {
            self.truncated = true;
            return;
        }
        self.mapping_requests.insert(request.key.clone(), request);
    }

    pub fn source_sha256(&self, uri: &str) -> Option<&str> {
        self.source_hashes.get(uri).map(String::as_str)
    }

    pub fn describe_credentials(&mut self, request: CredentialSelectionRequest) -> bool {
        let key = request.request.address.pointer.clone();
        let previous = self
            .credential_requests
            .get(&key)
            .map_or(0, |request| 1 + request.choices.len());
        let units = self
            .credential_units
            .saturating_sub(previous)
            .saturating_add(1)
            .saturating_add(request.choices.len());
        let previous_bytes = self
            .credential_requests
            .get(&key)
            .and_then(credential_bytes)
            .unwrap_or(0);
        let Some(request_bytes) = credential_bytes(&request) else {
            self.truncated = true;
            return false;
        };
        let bytes = self
            .credential_bytes
            .saturating_sub(previous_bytes)
            .saturating_add(request_bytes)
            .saturating_add(usize::from(
                previous == 0 && !self.credential_requests.is_empty(),
            ));
        // Count choices as well as requests: inherited arrays must not create
        // an unbounded Cartesian product in a structured review report.
        if units > self.max || bytes > self.max_credential_bytes {
            self.truncated = true;
            return false;
        }
        self.credential_units = units;
        self.credential_bytes = bytes;
        self.credential_requests.insert(key, request);
        true
    }

    pub fn validate_credential_selections(&mut self, sources: &Sources, config: &Config) {
        for key in config.credential_selections.keys() {
            if !self.credential_requests.contains_key(key) {
                self.record(sources, &sources.root, "", &[], "POSTMAN_AUTH_SELECTION_UNUSED", Classification::Blocked,
                    format!("Credential choice for request {key} no longer identifies an inherited credential-set array. Remove the stale choice or select credentials from a current request."), None);
            }
        }
    }

    pub fn describe_duplicate(&mut self, request: DuplicateRequest) {
        if self.duplicate_requests.len() >= self.max
            && !self.duplicate_requests.contains_key(&request.identity)
        {
            self.truncated = true;
            return;
        }
        self.duplicate_requests
            .insert(request.identity.clone(), request);
    }

    pub fn needs_review(&self) -> bool {
        self.decisions
            .iter()
            .any(|item| item.review_required && !item.reviewed)
    }

    pub fn finish(mut self) -> Vec<Decision> {
        self.decisions.sort_by(|left, right| {
            left.source
                .address
                .cmp(&right.source.address)
                .then(left.code.cmp(&right.code))
                .then(left.id.cmp(&right.id))
        });
        self.decisions
    }
}

fn credential_bytes(request: &CredentialSelectionRequest) -> Option<usize> {
    // Count JSON escaping exactly. The caller accounts for array delimiters.
    serde_json::to_vec(request).ok().map(|bytes| bytes.len())
}
