//! Selected documentation adapters share their host-owned snapshot.
#![cfg(feature = "docs")]
use roomkit::model::Metadata;
use roomkit_ai::{Tool, ToolError, ToolResult};
use roomkit_tools_docs::{
    DocsBackend, DocsListTool, DocsReadTool, DocsSearchTool, FileDocs, FileDocsLimits,
};
use roomkit_tools_roomkit::{DocsListToolset, DocsReadToolset, DocsSearchToolset};
use serde_json::{Value, json};
use std::sync::Arc;

#[expect(clippy::unwrap_used, reason = "fixture arguments are objects")]
fn arguments(arguments: Value) -> Metadata {
    serde_json::from_value(arguments).unwrap()
}
#[expect(clippy::panic, reason = "unexpected fixture output must fail")]
fn data(output: Result<ToolResult, ToolError>) -> Value {
    let Ok(ToolResult::Json(value)) = output else {
        panic!("expected JSON, got {output:?}")
    };
    value["data"].clone()
}
#[tokio::test]
async fn selected_tools_use_actual_schemas_and_share_the_host_snapshot() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("guide.md"),
        "# Guide\nUse expected_revision.\n",
    )
    .unwrap();
    let backend: Arc<dyn DocsBackend> =
        Arc::new(FileDocs::new(root.path(), None, FileDocsLimits::default()).unwrap());
    let lister = DocsListToolset::new(DocsListTool::new(Arc::clone(&backend)));
    let searcher = DocsSearchToolset::new(DocsSearchTool::new(Arc::clone(&backend)));
    let reader = DocsReadToolset::new(DocsReadTool::new(backend));
    assert_eq!(
        lister.definition().parameters,
        DocsListTool::definition().input_schema
    );
    assert_eq!(
        searcher.definition().parameters,
        DocsSearchTool::definition().input_schema
    );
    assert_eq!(
        reader.definition().parameters,
        DocsReadTool::definition().input_schema
    );
    let listing = data(
        lister
            .handler("docs_list".into(), arguments(json!({})))
            .await,
    );
    let hits = data(
        searcher
            .handler(
                "docs_search".into(),
                arguments(json!({"query":"expected_revision"})),
            )
            .await,
    );
    assert_eq!(hits["matches"][0]["document"], listing["documents"][0]);
    let document = &listing["documents"][0];
    let page = data(
        reader
            .handler(
                "docs_read".into(),
                arguments(json!({"document_id":document["document_id"], "expected_revision":document["revision"]})),
            )
            .await,
    );
    assert_eq!(page["lines"][1]["text"], "Use expected_revision.");
    assert!(matches!(
        reader.handler("read".into(), arguments(json!({}))).await,
        Err(ToolError::UnservedToolCall(_))
    ));
    assert!(matches!(
        reader
            .handler(
                "docs_read".into(),
                arguments(json!({"document_id":"../outside.md"}))
            )
            .await,
        Err(ToolError::Refused(_))
    ));
}
