//! Selected documentation adapters preserve their host-owned snapshot after close.
#![cfg(feature = "docs")]
use roomkit::model::{ChannelId, RoomId, ToolCall};
use roomkit_ai::{ToolContext, ToolError, ToolOutput, Toolset};
use roomkit_tools_docs::{
    DocsBackend, DocsListTool, DocsReadTool, DocsSearchTool, FileDocs, FileDocsLimits,
};
use roomkit_tools_roomkit::{DocsListToolset, DocsReadToolset, DocsSearchToolset};
use serde_json::{Value, json};
use std::sync::Arc;

#[expect(clippy::unwrap_used, reason = "fixture arguments are objects")]
fn call(name: &str, arguments: Value) -> ToolCall {
    ToolCall::new("id", name, serde_json::from_value(arguments).unwrap())
}
#[expect(clippy::panic, reason = "unexpected fixture output must fail")]
fn data(output: ToolOutput) -> Value {
    let ToolOutput::Json(value) = output else {
        panic!("expected JSON")
    };
    value["data"].clone()
}
#[tokio::test]
async fn selected_tools_use_actual_schemas_and_keep_snapshot_after_close() {
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
        lister.tools()[0].parameters,
        DocsListTool::definition().input_schema
    );
    assert_eq!(
        searcher.tools()[0].parameters,
        DocsSearchTool::definition().input_schema
    );
    assert_eq!(
        reader.tools()[0].parameters,
        DocsReadTool::definition().input_schema
    );
    let context = ToolContext::new(
        RoomId::new("docs").unwrap(),
        ChannelId::new("assistant").unwrap(),
    );
    let listing = data(
        lister
            .call(call("docs_list", json!({})), context.clone())
            .await
            .unwrap(),
    );
    lister.close().await;
    assert!(matches!(
        lister
            .call(call("docs_list", json!({})), context.clone())
            .await,
        Err(ToolError::Refused(_))
    ));
    let hits = data(
        searcher
            .call(
                call("docs_search", json!({"query":"expected_revision"})),
                context.clone(),
            )
            .await
            .unwrap(),
    );
    assert_eq!(hits["matches"][0]["document"], listing["documents"][0]);
    let document = &listing["documents"][0];
    let page = data(reader.call(call("docs_read", json!({"document_id":document["document_id"], "expected_revision":document["revision"]})), context.clone()).await.unwrap());
    assert_eq!(page["lines"][1]["text"], "Use expected_revision.");
    assert!(matches!(
        reader.call(call("read", json!({})), context.clone()).await,
        Err(ToolError::Refused(_))
    ));
    assert!(matches!(
        reader
            .call(
                call("docs_read", json!({"document_id":"../outside.md"})),
                context
            )
            .await,
        Err(ToolError::Refused(_))
    ));
}
