// SPDX-License-Identifier: GPL-3.0-or-later
//! External evidence gate: normal CI cannot substitute internal round trips.
#[path = "support/interchange_manifest.rs"]
mod reference;

#[test]
fn reference_manifest_rejects_missing_independence_and_empty_vectors() {
    assert!(
        reference::validate_manifest(
            &serde_json::json!({"schema_version":1,"producer":"uareu","vectors":[]})
        )
        .is_err()
    );
}
#[test]
fn reference_manifest_accepts_a_fully_identified_external_vector() {
    assert!(reference::validate_manifest(&serde_json::json!({"schema_version":1,"producer":"external-test-producer","vectors":[{"name":"one","format":"ansi378_2004","path":"input.fmr","sha256":"a".repeat(64),"valid":true,"expected":{},"source":"local independently produced fixture","license":"local test permission"}],"consumer":{"executable":"consumer","args":["--json"]}})).is_ok());
}
#[test]
#[ignore = "requires UAREU_INTERCHANGE_MANIFEST with independent vectors and consumer"]
fn independent_vectors_and_consumer_agree_with_supported_subsets() {
    reference::run_from_environment().unwrap();
}
