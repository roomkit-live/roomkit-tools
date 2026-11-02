//! Complete Rust/Python parity for image pixels, metadata and refusals.
#![forbid(unsafe_code)]

use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use roomkit_tools_core::ToolError;
use roomkit_tools_image::{ViewImageInput, ViewImageTool};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn shared_image_contracts() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../tests/fixtures/image.json")).unwrap();
    let root = tempfile::tempdir().unwrap();
    for (name, data) in fixture["files"].as_object().unwrap() {
        std::fs::write(
            root.path().join(name),
            STANDARD.decode(data.as_str().unwrap()).unwrap(),
        )
        .unwrap();
    }
    let mut reports = serde_json::Map::new();
    for case in fixture["cases"].as_array().unwrap() {
        let tool = ViewImageTool::new(root.path())
            .await
            .unwrap()
            .with_limits(
                serde_json::from_value(case.get("limits").cloned().unwrap_or(json!({}))).unwrap(),
            )
            .unwrap();
        let cancel = CancellationToken::new();
        if case["cancelled"] == true {
            cancel.cancel();
        }
        let result = match serde_json::from_value::<ViewImageInput>(case["input"].clone()) {
            Ok(input) => tool.view(input, &cancel).await,
            Err(error) => Err(ToolError::InvalidInput(error.to_string())),
        };
        let result = match result {
            Ok(output) => serde_json::to_value(output).unwrap(),
            Err(error) => json!({"error": match error {
                ToolError::InvalidInput(_) => "invalid_input", ToolError::AccessRefused(_) => "refused",
                ToolError::LimitExceeded(_) => "limit", ToolError::UnsupportedResource(_) => "unsupported",
                ToolError::Cancelled => "cancelled", ToolError::Io { .. } => "io", _ => "unknown",
            }}),
        };
        // The structured path and the metadata-only rendering name the image
        // relative to the workspace, never by host path.
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
    if let Ok(path) = std::env::var("ROOMKIT_IMAGE_RESULTS") {
        std::fs::write(
            path,
            serde_json::to_vec(&json!({"fixture":fixture,"reports":reports})).unwrap(),
        )
        .unwrap();
    }
}
