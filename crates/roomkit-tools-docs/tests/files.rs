//! Filesystem boundaries, snapshot revisions, query semantics and bounded pages.
#![cfg(feature = "files")]
use roomkit_tools_core::ToolError;
use roomkit_tools_docs::{
    DocsBackend, DocsListInput, DocsReadInput, DocsSearchInput, FileDocs, FileDocsLimits, decode,
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

/// `input` as JSON, to compare inputs field by field.
#[expect(clippy::unwrap_used, reason = "an input always serializes")]
fn json_of(input: impl serde::Serialize) -> serde_json::Value {
    serde_json::to_value(input).unwrap()
}

#[test]
fn inputs_built_in_rust_default_as_their_json_and_python() {
    // Python's FileDocs.list(), search(query=...) and read(document_id=...)
    // default to the offsets, limits and revisions the JSON contract does.
    let list: DocsListInput = serde_json::from_value(json!({})).unwrap();
    let search: DocsSearchInput = serde_json::from_value(json!({"query": "q"})).unwrap();
    let read: DocsReadInput = serde_json::from_value(json!({"document_id": "a.md"})).unwrap();
    assert_eq!(json_of(DocsListInput::default()), json_of(list));
    assert_eq!(json_of(DocsSearchInput::new("q")), json_of(search));
    assert_eq!(json_of(DocsReadInput::new("a.md")), json_of(read));
}

#[tokio::test]
async fn snapshot_refresh_and_revision_checked_pages() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("a.md"),
        "# Agent guide\nUse agent_followup for an idle agent.\nUse agent_send while running.\n",
    )
    .unwrap();
    std::fs::write(
        root.path().join("b.txt"),
        "Plans require expected_revision.\n",
    )
    .unwrap();
    let backend = FileDocs::new(root.path(), FileDocsLimits::default())
        .unwrap()
        .with_version("v1")
        .unwrap();
    let token = CancellationToken::new();
    let page = backend
        .list(&decode::<DocsListInput>(r#"{"limit":1}"#).unwrap(), &token)
        .await
        .unwrap();
    assert_eq!(page.total_documents, 2);
    assert_eq!(page.next_offset, Some(2));
    let next = backend
        .list(
            &decode(
                &json!({"offset":2,"expected_catalog_revision":page.catalog_revision}).to_string(),
            )
            .unwrap(),
            &token,
        )
        .await
        .unwrap();
    assert_eq!(next.documents[0].document_id, "b.txt");
    assert!(!next.truncated);
    let found = backend
        .search(
            &decode::<DocsSearchInput>(r#"{"query":"AGENT_FOLLOWUP idle"}"#).unwrap(),
            &token,
        )
        .await
        .unwrap();
    assert_eq!(found.matches[0].line, Some(2));
    assert_eq!(found.matched_documents, 1);
    let input=decode::<DocsReadInput>(&json!({"document_id":"a.md","offset":2,"limit":1,"expected_revision":found.matches[0].document.revision}).to_string()).unwrap();
    let result = backend.read(&input, &token).await.unwrap();
    assert_eq!(result.next_offset, Some(3));
    std::fs::write(
        root.path().join("a.md"),
        "# Changed\nDifferent documentation.\n",
    )
    .unwrap();
    assert_eq!(
        backend.clone().read(&input, &token).await.unwrap().lines[0].text,
        result.lines[0].text
    );
    let refreshed = FileDocs::new(root.path(), FileDocsLimits::default())
        .unwrap()
        .with_version("v1")
        .unwrap();
    assert!(matches!(
        refreshed.read(&input, &token).await,
        Err(ToolError::AccessRefused(_))
    ));
    assert!(matches!(
        refreshed
            .list(
                &decode(&json!({"expected_catalog_revision":page.catalog_revision}).to_string())
                    .unwrap(),
                &token
            )
            .await,
        Err(ToolError::AccessRefused(_))
    ));
    for id in [
        "../outside.md",
        "/etc/passwd",
        "https://example.com/private",
    ] {
        assert!(matches!(
            backend
                .read(
                    &decode(&json!({"document_id":id}).to_string()).unwrap(),
                    &token
                )
                .await,
            Err(ToolError::AccessRefused(_))
        ));
    }
}

#[tokio::test]
async fn scan_limits_fail_without_partial_catalogues_and_initialization_can_retry() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("valid.md"), "# Valid\n").unwrap();
    std::fs::write(root.path().join("invalid.txt"), [255, 0]).unwrap();
    let backend = FileDocs::new(root.path(), FileDocsLimits::default()).unwrap();
    let input = decode::<DocsListInput>("{}").unwrap();
    let token = CancellationToken::new();
    assert!(matches!(
        backend.list(&input, &token).await,
        Err(ToolError::UnsupportedResource(_))
    ));
    std::fs::remove_file(root.path().join("invalid.txt")).unwrap();
    assert_eq!(
        backend.list(&input, &token).await.unwrap().total_documents,
        1
    );
    for limits in [
        FileDocsLimits {
            max_document_bytes: 2,
            ..FileDocsLimits::default()
        },
        FileDocsLimits {
            max_total_bytes: 2,
            ..FileDocsLimits::default()
        },
        FileDocsLimits {
            max_total_lines: 1,
            ..FileDocsLimits::default()
        },
    ] {
        std::fs::write(root.path().join("valid.md"), "# Valid\nSecond line\n").unwrap();
        let backend = FileDocs::new(root.path(), limits).unwrap();
        assert!(matches!(
            backend.list(&input, &token).await,
            Err(ToolError::LimitExceeded(_))
        ));
    }
    std::fs::write(root.path().join("second.md"), "Second").unwrap();
    for limits in [
        FileDocsLimits {
            max_documents: 1,
            ..FileDocsLimits::default()
        },
        FileDocsLimits {
            max_entries: 1,
            ..FileDocsLimits::default()
        },
    ] {
        assert!(matches!(
            FileDocs::new(root.path(), limits)
                .unwrap()
                .list(&input, &token)
                .await,
            Err(ToolError::LimitExceeded(_))
        ));
    }
    std::fs::create_dir_all(root.path().join("a/b")).unwrap();
    assert!(matches!(
        FileDocs::new(
            root.path(),
            FileDocsLimits {
                max_depth: 1,
                ..FileDocsLimits::default()
            }
        )
        .unwrap()
        .list(&input, &token)
        .await,
        Err(ToolError::LimitExceeded(_))
    ));
}

#[tokio::test]
async fn line_pages_clip_only_between_lines_and_excerpts_preserve_utf8() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("page.md"),
        format!("# Match\n{}\n{}\n", "é".repeat(900), "x".repeat(2500)),
    )
    .unwrap();
    let backend = FileDocs::new(
        root.path(),
        FileDocsLimits {
            max_read_bytes: 4096,
            ..FileDocsLimits::default()
        },
    )
    .unwrap();
    let token = CancellationToken::new();
    let result = backend
        .search(&decode(&json!({"query":"é"}).to_string()).unwrap(), &token)
        .await
        .unwrap();
    assert_eq!(result.matches[0].text.len(), 1024);
    assert_eq!(result.matches[0].omitted_bytes, 776);
    let page = backend
        .read(&decode(r#"{"document_id":"page.md"}"#).unwrap(), &token)
        .await
        .unwrap();
    assert_eq!(page.next_offset, Some(3));
    assert_eq!(page.lines[1].text.len(), 1800);
    let next = backend
        .read(
            &decode(r#"{"document_id":"page.md","offset":3}"#).unwrap(),
            &token,
        )
        .await
        .unwrap();
    assert_eq!(next.lines[0].text.len(), 2500);
    assert!(!next.truncated);
    std::fs::write(root.path().join("long.txt"), "x".repeat(5000)).unwrap();
    let fresh = FileDocs::new(
        root.path(),
        FileDocsLimits {
            max_read_bytes: 4096,
            ..FileDocsLimits::default()
        },
    )
    .unwrap();
    assert!(matches!(
        fresh
            .read(&decode(r#"{"document_id":"long.txt"}"#).unwrap(), &token)
            .await,
        Err(ToolError::LimitExceeded(_))
    ));
    assert!(decode::<DocsReadInput>(r#"{"document_id":"page.md","offset":0}"#).is_err());
    assert!(decode::<DocsSearchInput>(r#"{"query":" "}"#).is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn symlinks_and_hidden_files_are_not_part_of_the_catalogue() {
    let root = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    std::fs::write(other.path().join("private.md"), "Private").unwrap();
    std::os::unix::fs::symlink(
        other.path().join("private.md"),
        root.path().join("alias.md"),
    )
    .unwrap();
    std::os::unix::fs::symlink(other.path(), root.path().join("linked")).unwrap();
    std::fs::create_dir(root.path().join(".hidden")).unwrap();
    std::fs::write(root.path().join(".hidden/private.md"), "Hidden").unwrap();
    std::fs::write(root.path().join("README.md"), "Visible").unwrap();
    let backend = FileDocs::new(root.path(), FileDocsLimits::default()).unwrap();
    let token = CancellationToken::new();
    assert_eq!(
        backend
            .list(&decode("{}").unwrap(), &token)
            .await
            .unwrap()
            .total_documents,
        1
    );
    assert!(matches!(
        backend
            .read(&decode(r#"{"document_id":"alias.md"}"#).unwrap(), &token)
            .await,
        Err(ToolError::AccessRefused(_))
    ));
}
