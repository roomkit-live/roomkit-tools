//! Selected documentation adapters share their host-owned snapshot.
#![cfg(feature = "docs")]
use roomkit::model::Metadata;
use roomkit_ai::{Tool, ToolError, ToolResult};
use roomkit_tools_docs::{
    DocsBackend, DocsFuture, DocsListInput, DocsListResult, DocsListTool, DocsReadInput,
    DocsReadResult, DocsReadTool, DocsSearchInput, DocsSearchResult, DocsSearchTool, FileDocs,
    FileDocsLimits,
};
use roomkit_tools_roomkit::{DocsListToolset, DocsReadToolset, DocsSearchToolset};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, PoisonError};
use tokio_util::sync::CancellationToken;

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
        Arc::new(FileDocs::new(root.path(), FileDocsLimits::default()).unwrap());
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

/// A host backend whose calls never end, keeping the token `list` was given.
#[derive(Debug, Default)]
struct Pending {
    token: Arc<Mutex<Option<CancellationToken>>>,
}

impl DocsBackend for Pending {
    fn list<'a>(
        &'a self,
        _: &'a DocsListInput,
        cancel: &'a CancellationToken,
    ) -> DocsFuture<'a, DocsListResult> {
        *self.token.lock().unwrap_or_else(PoisonError::into_inner) = Some(cancel.clone());
        Box::pin(std::future::pending())
    }
    fn search<'a>(
        &'a self,
        _: &'a DocsSearchInput,
        _: &'a CancellationToken,
    ) -> DocsFuture<'a, DocsSearchResult> {
        Box::pin(std::future::pending())
    }
    fn read<'a>(
        &'a self,
        _: &'a DocsReadInput,
        _: &'a CancellationToken,
    ) -> DocsFuture<'a, DocsReadResult> {
        Box::pin(std::future::pending())
    }
}

/// The docs facade hands the adapter's token to the host's backend as it is:
/// only the adapter cancels it when RoomKit drops the call.
#[tokio::test]
async fn dropping_a_call_cancels_the_token_the_host_backend_holds() {
    let backend = Pending::default();
    let token = Arc::clone(&backend.token);
    let tool = DocsListToolset::new(DocsListTool::new(Arc::new(backend)));
    let mut call = tool.handler("docs_list".into(), Metadata::new());
    // Poll once, so the backend receives its token.
    let answered = tokio::select! {
        biased;
        _ = &mut call => true,
        () = std::future::ready(()) => false,
    };
    assert!(!answered, "the backend never answers");
    let held = token.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let Some(held) = held else {
        unreachable!("the backend received a token");
    };
    assert!(!held.is_cancelled());
    drop(call);
    assert!(
        held.is_cancelled(),
        "dropping the call cancels the backend's token"
    );
}
