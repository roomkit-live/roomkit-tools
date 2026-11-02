//! Select documentation tools independently over one immutable host-owned snapshot.
#![forbid(unsafe_code)]
#![expect(clippy::print_stdout, reason = "standalone demonstration output")]
use roomkit_tools_docs::{
    DocsBackend, DocsListInput, DocsListTool, DocsReadInput, DocsReadTool, DocsSearchInput,
    DocsSearchTool, FileDocs, FileDocsLimits,
};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    std::fs::write(
        root.path().join("agents.md"),
        include_str!("fixtures/documentation/agents.md"),
    )?;
    let backend: Arc<dyn DocsBackend> =
        Arc::new(FileDocs::new(root.path(), FileDocsLimits::default())?.with_version("demo-v1")?);
    let cancel = CancellationToken::new();
    let catalogue = DocsListTool::new(Arc::clone(&backend))
        .execute(DocsListInput::default(), &cancel)
        .await?
        .data;
    assert_eq!(catalogue.total_documents, 1);
    let matches = DocsSearchTool::new(Arc::clone(&backend))
        .execute(
            DocsSearchInput {
                expected_catalog_revision: Some(catalogue.catalog_revision),
                ..DocsSearchInput::new("agent_followup idle")
            },
            &cancel,
        )
        .await?
        .data;
    let hit = matches
        .matches
        .first()
        .ok_or("missing expected documentation")?;
    assert_eq!(hit.line, Some(5));
    let page = DocsReadTool::new(backend)
        .execute(
            DocsReadInput {
                offset: 5,
                limit: 2,
                expected_revision: Some(hit.document.revision.clone()),
                ..DocsReadInput::new(hit.document.document_id.clone())
            },
            &cancel,
        )
        .await?
        .data;
    assert_eq!(page.lines.len(), 2);
    assert_eq!(page.document.revision, hit.document.revision);
    assert_eq!(page.next_offset, Some(7));
    println!("Verified: catalogue discovery, literal search and revision-checked source lines.");
    Ok(())
}
