//! Shared syntax cases compare complete Rust/Python envelopes and source ranges.
#![forbid(unsafe_code)]

use std::path::Path;

use roomkit_tools_code::{CodeOutlineInput, CodeOutlineTool};
use roomkit_tools_core::Error;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn shared_outline_contracts() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../tests/fixtures/outline.json")).unwrap();
    let root = tempfile::tempdir().unwrap();
    for (path, text) in fixture["files"].as_object().unwrap() {
        std::fs::write(root.path().join(path), text.as_str().unwrap()).unwrap();
    }
    let mut reports = serde_json::Map::new();
    for case in fixture["cases"].as_array().unwrap() {
        let tool = CodeOutlineTool::new(root.path())
            .await
            .unwrap()
            .with_limits(
                serde_json::from_value(case.get("limits").cloned().unwrap_or(json!({}))).unwrap(),
            )
            .unwrap();
        let token = CancellationToken::new();
        if case["cancelled"] == true {
            token.cancel();
        }
        let result = match serde_json::from_value::<CodeOutlineInput>(case["input"].clone()) {
            Ok(input) => tool.outline(input, &token).await,
            Err(error) => Err(Error::InvalidInput(error.to_string())),
        };
        let result = match result {
            Ok(output) => serde_json::to_value(output).unwrap(),
            Err(error) => json!({"error": match error {
                Error::InvalidInput(_) => "invalid_input", Error::Refused(_) => "refused",
                Error::LimitExceeded(_) => "limit", Error::Unsupported(_) => "unsupported",
                Error::Cancelled => "cancelled", Error::Io {..} => "io", _ => "unknown",
            }}),
        };
        // Results name the file relative to the workspace, never by host path.
        assert!(
            !result.to_string().contains(root.path().to_str().unwrap()),
            "{}: {result}",
            case["name"]
        );
        if let Some(path) = result.pointer("/data/path") {
            assert!(!Path::new(path.as_str().unwrap()).is_absolute(), "{path}");
        }
        for (pointer, expected) in case["expect"].as_object().unwrap() {
            assert_eq!(
                result.pointer(pointer),
                Some(expected),
                "{}: {result}",
                case["name"]
            );
        }
        reports.insert(case["name"].as_str().unwrap().into(), result);
    }
    if let Ok(path) = std::env::var("ROOMKIT_OUTLINE_RESULTS") {
        std::fs::write(
            path,
            serde_json::to_vec(&json!({"fixture":fixture,"reports":reports})).unwrap(),
        )
        .unwrap();
    }
}
