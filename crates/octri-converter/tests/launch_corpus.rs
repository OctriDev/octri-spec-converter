//! Representative launch adapters, with explicit expected semantics and losses.
use std::collections::{BTreeMap, BTreeSet};

use octri_converter::{Classification, Config, ConversionRequest, Status, convert};
use octri_parser::Input;
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Fixture {
    name: String,
    source: Value,
    #[serde(default)]
    config: Config,
    #[serde(default)]
    files: BTreeMap<String, String>,
    expected: BTreeMap<String, Value>,
    #[serde(default)]
    expected_single_auth: BTreeMap<String, ExpectedAuth>,
    #[serde(default)]
    absent: Vec<String>,
    #[serde(default)]
    decision_codes: BTreeSet<String>,
    #[serde(default)]
    allowed_losses: BTreeSet<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExpectedAuth {
    scheme: Value,
    scopes: Vec<String>,
}

fn verify_corpus(adapter: &str, text: &str) -> Result<(), Box<dyn std::error::Error>> {
    let fixtures: Vec<Fixture> = serde_json::from_str(text)?;
    assert!(
        fixtures.len() >= 20,
        "{adapter}: launch corpus below product gate"
    );
    let mut names = BTreeSet::new();
    for fixture in fixtures {
        assert!(names.insert(fixture.name.clone()), "duplicate fixture name");
        assert!(
            !fixture.expected.is_empty(),
            "{}: missing oracle",
            fixture.name
        );
        let mut request = ConversionRequest {
            input: Input {
                uri: format!("memory://launch/{adapter}/{}.json", fixture.name),
                content: Some(fixture.source.to_string()),
                generated: false,
            },
            files: fixture.files,
            config: fixture.config,
        };
        let report = convert(&request)?;
        assert!(report.valid, "{}: {:?}", fixture.name, report.diagnostics);
        assert_ne!(
            report.status,
            Status::Blocked,
            "{}: {:?}",
            fixture.name,
            report.decisions
        );
        let document = report.document.as_ref().ok_or("missing output")?;
        for (pointer, expected) in fixture.expected {
            assert_eq!(
                document.pointer(&pointer),
                Some(&expected),
                "{}: {pointer}",
                fixture.name
            );
        }
        for pointer in fixture.absent {
            assert!(
                document.pointer(&pointer).is_none(),
                "{}: unexpected {pointer}",
                fixture.name
            );
        }
        for (pointer, expected) in fixture.expected_single_auth {
            let alternatives = document
                .pointer(&format!("{pointer}/security"))
                .and_then(Value::as_array)
                .ok_or("missing security requirement")?;
            assert_eq!(alternatives.len(), 1, "{}: auth alternatives", fixture.name);
            let requirement = alternatives[0].as_object().ok_or("invalid security")?;
            assert_eq!(requirement.len(), 1, "{}: auth conjunction", fixture.name);
            let (name, scopes) = requirement.iter().next().ok_or("empty security")?;
            assert_eq!(
                scopes,
                &serde_json::json!(expected.scopes),
                "{}: scopes",
                fixture.name
            );
            assert_eq!(
                document["components"]["securitySchemes"].get(name),
                Some(&expected.scheme),
                "{}: security scheme contract",
                fixture.name
            );
        }
        for code in fixture.decision_codes {
            assert!(
                report
                    .decisions
                    .iter()
                    .any(|decision| decision.code == code),
                "{}: missing {code}",
                fixture.name
            );
        }
        for decision in &report.decisions {
            if matches!(
                decision.classification,
                Classification::Omitted | Classification::Blocked
            ) {
                assert!(
                    fixture.allowed_losses.contains(&decision.code),
                    "{}: unexpected loss {} at {}",
                    fixture.name,
                    decision.code,
                    decision.source.address.pointer
                );
            }
        }
        // Inference remains reviewable. Only this test's explicit acknowledgement
        // may complete its handoff; no production defaults accept these decisions.
        request.config.accepted_decisions = report
            .decisions
            .iter()
            .filter(|decision| decision.review_required)
            .map(|decision| decision.id.clone())
            .collect();
        let reviewed = convert(&request)?;
        assert_eq!(
            reviewed.status,
            Status::Complete,
            "{}: {:?}",
            fixture.name,
            reviewed.decisions
        );
        assert!(
            reviewed.handoff_allowed,
            "{}: handoff unavailable",
            fixture.name
        );
        assert_eq!(
            reviewed.document, report.document,
            "{}: review changed output",
            fixture.name
        );
    }
    Ok(())
}

#[test]
fn swagger_launch_corpus_preserves_expected_http_contracts()
-> Result<(), Box<dyn std::error::Error>> {
    verify_corpus(
        "swagger",
        include_str!("../../../fixtures/launch/swagger.json"),
    )
}

#[test]
fn postman_launch_corpus_preserves_expected_observations_and_review()
-> Result<(), Box<dyn std::error::Error>> {
    verify_corpus(
        "postman",
        include_str!("../../../fixtures/launch/postman.json"),
    )
}
