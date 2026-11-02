//! Complete Rust envelopes for installed Python parity checks.
#![cfg(feature = "files")]
use roomkit_tools_docs::{
    DocsBackend, DocsListTool, DocsReadTool, DocsSearchTool, FileDocs, FileDocsLimits, decode,
};
use serde_json::Value;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
#[tokio::test]
async fn shared_documentation_contracts() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/documentation.json")).unwrap();
    let root = tempfile::tempdir().unwrap();
    for (path, text) in fixture["files"].as_object().unwrap() {
        std::fs::write(root.path().join(path), text.as_str().unwrap()).unwrap();
    }
    let backend: Arc<dyn DocsBackend> = Arc::new(
        FileDocs::new(root.path(), FileDocsLimits::default())
            .unwrap()
            .with_version("fixture-v1")
            .unwrap(),
    );
    let token = CancellationToken::new();
    let mut results = Vec::new();
    for case in fixture["cases"].as_array().unwrap() {
        let input = case["arguments"].to_string();
        macro_rules! call {
            ($tool:ident) => {
                serde_json::to_value(
                    $tool::new(Arc::clone(&backend))
                        .call(decode(&input).unwrap(), &token)
                        .await
                        .unwrap(),
                )
                .unwrap()
            };
        }
        results.push(match case["tool"].as_str().unwrap() {
            "docs_list" => call!(DocsListTool),
            "docs_search" => call!(DocsSearchTool),
            "docs_read" => call!(DocsReadTool),
            _ => panic!("invalid fixture"),
        });
    }
    assert_eq!(results[0]["data"]["total_documents"], 3);
    // Offsets start at one, as list, glob and grep's: the third document.
    assert_eq!(results[0]["data"]["offset"], 1);
    assert_eq!(results[0]["data"]["next_offset"], 3);
    assert_eq!(results[1]["data"]["offset"], 3);
    assert_eq!(results[1]["data"]["documents"].as_array().unwrap().len(), 1);
    assert_eq!(results[1]["data"]["next_offset"], Value::Null);
    assert_eq!(results[2]["data"]["matches"][0]["line"], 2);
    assert_eq!(results[3]["data"]["matched_documents"], 0);
    assert_eq!(results[4]["data"]["next_offset"], 3);
    assert_eq!(results[5]["data"]["lines"], serde_json::json!([]));
    if let Ok(path) = std::env::var("ROOMKIT_DOCS_RESULTS") {
        std::fs::write(path, serde_json::to_vec(&results).unwrap()).unwrap();
    }
}

#[test]
fn a_result_offset_starts_at_one() {
    use roomkit_tools_docs::{DocsListInput, DocsSearchInput};
    assert!(decode::<DocsListInput>(r#"{"offset":0}"#).is_err());
    assert!(decode::<DocsSearchInput>(r#"{"query":"q","offset":0}"#).is_err());
    assert_eq!(DocsListInput::default().offset.get(), 1);
    assert_eq!(DocsSearchInput::new("q").offset.get(), 1);
    let list = DocsListTool::definition();
    assert_eq!(list.input_schema["properties"]["offset"]["minimum"], 1);
    assert_eq!(list.input_schema["properties"]["offset"]["default"], 1);
}
