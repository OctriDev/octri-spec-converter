//! Deterministic policy evaluation. Integrity cannot be disabled or suppressed.

use crate::{
    AuditError, Baseline, Config, Finding, REPORT_VERSION, RULESET_VERSION, Result, catalogue,
};

pub const SDK_LANGUAGES: &[&str] = &[
    "typescript",
    "python",
    "rust",
    "dart",
    "ruby",
    "php",
    "swift",
    "java",
    "kotlin",
    "go",
];

pub fn validate(config: &Config) -> Result<()> {
    if config.profiles.len() > 32
        || config.ignored.len() > 10_000
        || config.exclusions.len() > 10_000
    {
        return Err(AuditError::new(
            "INVALID_CONFIG",
            "Policy supports at most 32 profiles, 10000 ignored keys and 10000 exclusions",
        ));
    }
    let limits = &config.limits;
    if !(1..=24 * 1024 * 1024).contains(&limits.max_document_bytes)
        || !(1..=64 * 1024 * 1024).contains(&limits.max_total_bytes)
        || !(1..=100).contains(&limits.max_documents)
        || !(1..=1_000_000).contains(&limits.max_nodes)
        || !(1..=128).contains(&limits.max_depth)
    {
        return Err(AuditError::new(
            "INVALID_CONFIG",
            "Limits must be positive and at most 24 MiB/file, 64 MiB total, 100 files, 1000000 nodes and depth 128",
        ));
    }
    if config.max_findings == 0 || config.max_findings > 10_000 {
        return Err(AuditError::new(
            "INVALID_CONFIG",
            "maxFindings must be between 1 and 10000",
        ));
    }
    if config.profiles.is_empty() {
        return Err(AuditError::new(
            "INVALID_CONFIG",
            "Select at least one compatibility profile",
        ));
    }
    for profile in &config.profiles {
        let sdk_language = profile
            .strip_prefix("sdk-")
            .is_some_and(|language| SDK_LANGUAGES.contains(&language));
        if !matches!(profile.as_str(), "docs" | "sdk" | "mcp") && !sdk_language {
            return Err(AuditError::new(
                "UNKNOWN_PROFILE",
                format!("Unknown compatibility profile: {profile}"),
            ));
        }
    }
    for rule in config.rules.keys() {
        if rule == "spec-integrity" {
            return Err(AuditError::new(
                "INVALID_CONFIG",
                "The integrity gate cannot be disabled or downgraded",
            ));
        }
        if catalogue::find(rule).is_none() {
            return Err(AuditError::new(
                "UNKNOWN_RULE",
                format!("Unknown rule: {rule}"),
            ));
        }
    }
    if let Some(date) = &config.as_of
        && !valid_date(date)
    {
        return Err(AuditError::new(
            "INVALID_CONFIG",
            "asOf must be a valid YYYY-MM-DD date",
        ));
    }
    for exclusion in &config.exclusions {
        if exclusion.rule_id == "spec-integrity" || catalogue::find(&exclusion.rule_id).is_none() {
            return Err(AuditError::new(
                "INVALID_CONFIG",
                "Exclusions must reference a known, suppressible rule",
            ));
        }
        if exclusion.reason.trim().is_empty() {
            return Err(AuditError::new(
                "INVALID_CONFIG",
                "Every exclusion needs a reason",
            ));
        }
        if let Some(expiry) = &exclusion.expires_on
            && (!valid_date(expiry) || config.as_of.is_none())
        {
            return Err(AuditError::new(
                "INVALID_CONFIG",
                "Expiring exclusions require valid expiresOn and explicit asOf dates",
            ));
        }
    }
    if let Some(baseline) = &config.baseline {
        validate_baseline(baseline)?;
    }
    if config.fail_on_new && config.baseline.is_none() {
        return Err(AuditError::new(
            "INVALID_CONFIG",
            "failOnNew requires a version-matched baseline",
        ));
    }
    Ok(())
}

pub fn validate_baseline(baseline: &Baseline) -> Result<()> {
    if baseline.fingerprints.len() > 10_000
        || baseline
            .fingerprints
            .iter()
            .any(|value| value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err(AuditError::new(
            "INVALID_CONFIG",
            "Baseline supports at most 10000 SHA-256 fingerprints",
        ));
    }
    if baseline.schema_version != REPORT_VERSION || baseline.ruleset_version != RULESET_VERSION {
        return Err(AuditError::new(
            "VERSION_MISMATCH",
            "Baseline schema and ruleset versions must match this engine",
        ));
    }
    Ok(())
}

pub fn exclusion_reason(finding: &Finding, config: &Config) -> Option<String> {
    if !finding.ignorable {
        return None;
    }
    if config.ignored.contains(&finding.key) {
        return Some("Legacy project ignore".into());
    }
    config
        .exclusions
        .iter()
        .find(|exclusion| {
            exclusion.rule_id == finding.rule_id
                && exclusion.key.as_ref().is_none_or(|key| key == &finding.key)
                && exclusion
                    .uri
                    .as_ref()
                    .is_none_or(|uri| uri == &finding.location.address.uri)
                && exclusion
                    .pointer
                    .as_ref()
                    .is_none_or(|pointer| pointer == &finding.location.address.pointer)
                && exclusion
                    .expires_on
                    .as_ref()
                    .is_none_or(|expiry| config.as_of.as_ref().is_some_and(|date| date <= expiry))
        })
        .map(|exclusion| exclusion.reason.clone())
}

pub fn baseline(finding: &Finding, config: &Config) -> bool {
    finding.ignorable
        && config
            .baseline
            .as_ref()
            .is_some_and(|baseline| baseline.fingerprints.contains(&finding.fingerprint))
}

fn valid_date(value: &str) -> bool {
    let parts: Vec<_> = value.split('-').collect();
    if parts.len() != 3 || parts[0].len() != 4 || parts[1].len() != 2 || parts[2].len() != 2 {
        return false;
    }
    let (Ok(year), Ok(month), Ok(day)) = (
        parts[0].parse::<u32>(),
        parts[1].parse::<u32>(),
        parts[2].parse::<u32>(),
    ) else {
        return false;
    };
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => 0,
    };
    year > 0 && day > 0 && day <= days
}
