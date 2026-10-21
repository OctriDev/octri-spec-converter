#![allow(clippy::unwrap_used)]

use octri_auditor::{AuditRequest, Finding, audit, fixes};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

fn document(id: &str) -> Value {
    let mut document: Value =
        serde_json::from_str(include_str!("../../../fixtures/clean.json")).unwrap();
    document["paths"]["/pets"]["get"]["operationId"] = json!(id);
    document
}

fn findings(document: Value, rule: &str) -> Vec<Finding> {
    let request: AuditRequest = serde_json::from_value(json!({
        "input": {"uri": "memory://audit/legacy.json", "content": document.to_string()}
    }))
    .unwrap();
    audit(&request)
        .unwrap()
        .findings
        .into_iter()
        .filter(|finding| finding.rule_id == rule)
        .collect()
}

fn rename(document: Value) -> Option<String> {
    findings(document, "operation-id-naming")
        .first()
        .and_then(|finding| finding.fix.as_ref())
        .and_then(|fix| fix.edits.first())
        .and_then(|edit| edit.after.as_str())
        .map(str::to_owned)
}

#[test]
fn semantic_verbs_and_noun_boundaries_are_preserved() {
    for id in [
        "findPetsByTags",
        "findPetsByStatus",
        "getPetsByTags",
        "fetchPetsByTags",
        "retrievePets",
        "cancelPet",
        "activatePet",
        "getFindings",
        "getFinder",
        "getSearchResults",
        "getReadReceipts",
        "getLookupTable",
        "getListItems",
    ] {
        assert!(
            findings(document(id), "operation-id-naming").is_empty(),
            "{id}"
        );
    }
}

#[test]
fn unfamiliar_actions_never_gain_an_http_verb() {
    for id in [
        "loginUser",
        "logoutUser",
        "placeOrder",
        "onboardEmployee",
        "provisionCluster",
        "reconcileInvoice",
        "frobnicateResource",
        "provisionGetToken",
        "reconcileReadReceipt",
        "onboardUserGet",
        "LoginUser",
        "LogoutUser",
        "PlaceOrder",
        "ProvisionGetToken",
        "login_user",
        "LOGOUT_USER",
        "place-order",
        "users.loginUser",
        "root",
        "petsByStatus",
    ] {
        for method in [
            "get", "post", "put", "patch", "delete", "head", "options", "trace",
        ] {
            let mut value = document(id);
            let operation = value["paths"]["/pets"]["get"].take();
            value["paths"]["/pets"]
                .as_object_mut()
                .unwrap()
                .remove("get");
            value["paths"]["/pets"][method] = operation;
            assert!(
                findings(value, "operation-id-naming").is_empty(),
                "{method} {id}: an unfamiliar word is not evidence of a missing action"
            );
        }
    }
}

#[test]
fn authored_actions_are_not_mistaken_for_namespace_prefixes() {
    for (id, tag) in [
        ("provisionGetToken", "provision"),
        ("reconcileReadReceipt", "Reconcile"),
        ("onboardUserGet", "Onboard"),
        ("ProvisionGetToken", "Provision"),
        ("ReconcileReadReceipt", "Reconcile"),
        ("OnboardUserGet", "Onboard"),
        ("PROVISION_GET_TOKEN", "PROVISION"),
        ("provision-get-token", "provision"),
    ] {
        let mut value = document(id);
        value["paths"]["/pets"]["get"]["tags"] = json!([tag]);
        assert!(findings(value, "operation-id-naming").is_empty(), "{id}");
    }
}

#[test]
fn namespace_cleanup_preserves_the_authored_leading_action() {
    let mut value = document("cancelOrderOrderRead");
    value["paths"]["/pets"]["get"]["tags"] = json!(["Order"]);
    assert_eq!(rename(value).as_deref(), Some("cancelOrderRead"));
}

#[test]
fn redundant_leading_actions_are_repaired_without_an_http_prefix() {
    for (before, after) in [
        ("getFindPetsByTags", "findPetsByTags"),
        ("findFindPets", "findPets"),
        ("getGetPets", "getPets"),
    ] {
        assert_eq!(rename(document(before)).as_deref(), Some(after), "{before}");
    }
}

#[test]
fn existing_schema_conventions_guide_repairs_but_do_not_rename_valid_ids() {
    let mut value = document("getFindPetsByTags");
    let operation = value["paths"]["/pets"]["get"].clone();
    for (path, id) in [
        ("/pets/recent", "getRecentPets"),
        ("/pets/active", "getActivePets"),
    ] {
        value["paths"][path]["get"] = operation.clone();
        value["paths"][path]["get"]["operationId"] = json!(id);
    }
    assert_eq!(rename(value.clone()).as_deref(), Some("getPetsByTags"));
    value["paths"]["/pets"]["get"]["operationId"] = json!("findPetsByTags");
    assert!(findings(value, "operation-id-naming").is_empty());
}

#[test]
fn ambiguous_namespace_and_acronym_prefixes_are_preserved() {
    for (id, namespace) in [
        ("ActionsActionsGet", "Actions"),
        ("ActionsGet", "Actions"),
        ("PetsUsersDelete", "Pets"),
        ("UsersUserDelete", "Users"),
        ("HTMLCheckGet", "HTML"),
        ("PetsFindByTags", "Pets"),
        ("PetsByTagsFind", "Pets"),
    ] {
        let mut value = document(id);
        value["paths"]["/pets"]["get"]["tags"] = json!([namespace]);
        assert!(findings(value, "operation-id-naming").is_empty(), "{id}");
    }
}

#[test]
fn repairs_never_suggest_an_identifier_already_in_the_document() {
    let mut value = document("getFindPetsByTags");
    value["paths"]["/other"]["get"] = value["paths"]["/pets"]["get"].clone();
    value["paths"]["/other"]["get"]["operationId"] = json!("findPetsByTags");
    assert!(findings(value, "operation-id-naming").is_empty());
}

#[test]
fn one_operation_cannot_establish_a_schema_convention() {
    let mut value = document("getFindPetsByTags");
    value["paths"]["/other"]["get"] = value["paths"]["/pets"]["get"].clone();
    value["paths"]["/other"]["get"]["operationId"] = json!("getOtherPets");
    assert_eq!(rename(value).as_deref(), Some("findPetsByTags"));
}

#[test]
fn selecting_multiple_repairs_does_not_create_duplicate_identifiers() {
    let mut value = document("getFindPetsByTags");
    value["paths"]["/other"]["get"] = value["paths"]["/pets"]["get"].clone();
    value["paths"]["/other"]["get"]["operationId"] = json!("findFindPetsByTags");
    let request: AuditRequest = serde_json::from_value(json!({
        "input": {"uri": "memory://audit/repairs.json", "content": value.to_string()}
    }))
    .unwrap();
    let report = audit(&request).unwrap();
    let keys: Vec<_> = report
        .findings
        .iter()
        .filter(|finding| finding.rule_id == "operation-id-naming")
        .map(|finding| finding.key.clone())
        .collect();
    assert_eq!(
        keys.len(),
        1,
        "competing proposals must reserve their identifier"
    );
    let result = fixes::apply(&fixes::FixRequest {
        request,
        keys,
        expected_files: fixes::hashes(&report),
        inputs: BTreeMap::new(),
    })
    .unwrap();
    let patched: Value =
        serde_json::from_str(result.revised.input.content.as_deref().unwrap()).unwrap();
    let ids: Vec<_> = patched["paths"]
        .as_object()
        .unwrap()
        .values()
        .filter_map(|path| path["get"]["operationId"].as_str())
        .collect();
    assert_eq!(ids.iter().collect::<BTreeSet<_>>().len(), ids.len());
}

#[test]
fn empty_security_requirements_do_not_define_an_authentication_scheme() {
    let mut value = document("findPets");
    value["components"]
        .as_object_mut()
        .unwrap()
        .remove("securitySchemes");
    value["security"] = json!([]);
    value["paths"]["/pets"]["get"]["security"] = json!([]);
    assert_eq!(findings(value.clone(), "security-declared").len(), 1);
    value["components"]["securitySchemes"] = json!({"token": {"type": "http", "scheme": "bearer"}});
    assert!(findings(value, "security-declared").is_empty());
}
