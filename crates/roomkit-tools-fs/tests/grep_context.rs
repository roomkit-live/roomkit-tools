//! Context belongs to a matching line and is never silently cut by pagination.

use std::num::NonZeroUsize;

use roomkit_tools_core::ToolError;
use roomkit_tools_fs::{GrepInput, GrepLimits, GrepTool};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn context_preserves_source_lines_and_match_offsets_across_pages() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("source"),
        "\u{feff}first\r\nhit π\r\nhit two\r\nlast\r\n",
    )
    .unwrap();
    let tool = GrepTool::new(root.path()).await.unwrap();
    let cancel = CancellationToken::new();
    let mut input = GrepInput::new("hit");
    input.context_before = 2;
    input.context_after = 2;
    input.limit = NonZeroUsize::new(1);
    let first = tool.grep(input.clone(), &cancel).await.unwrap();
    assert_eq!(first.data.matches.len(), 1);
    let found = &first.data.matches[0];
    assert_eq!((found.line, found.text.as_str()), (2, "hit π"));
    assert_eq!(
        found
            .context_before
            .iter()
            .map(|l| (l.number, l.text.as_str()))
            .collect::<Vec<_>>(),
        [(1, "first")]
    );
    assert_eq!(
        found
            .context_after
            .iter()
            .map(|l| (l.number, l.text.as_str()))
            .collect::<Vec<_>>(),
        [(3, "hit two"), (4, "last")]
    );
    assert_eq!(
        first.text,
        "\"source\"-1- first\n\"source\":2: hit π\n\"source\"-3- hit two\n\"source\"-4- last\n"
    );
    assert_eq!(first.data.next_offset, Some(2));
    input.offset = NonZeroUsize::new(2).unwrap();
    let second = tool.grep(input, &cancel).await.unwrap();
    let found = &second.data.matches[0];
    assert_eq!(found.line, 3);
    assert_eq!(found.context_before.len(), 2);
    assert_eq!(found.context_after.len(), 1);
    assert!(!second.data.truncated);
    assert_eq!(second.data.next_offset, None);
}

#[tokio::test]
async fn context_never_crosses_files_and_absent_context_keeps_the_old_rendering() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a"), "last hit\n").unwrap();
    std::fs::write(root.path().join("b"), "first hit\n").unwrap();
    let tool = GrepTool::new(root.path()).await.unwrap();
    let cancel = CancellationToken::new();
    let mut input = GrepInput::new("hit");
    let original = tool.grep(input.clone(), &cancel).await.unwrap();
    assert_eq!(original.text, "\"a\":1: last hit\n\"b\":1: first hit\n");
    assert!(
        serde_json::to_value(&original).unwrap()["data"]["matches"][0]
            .get("context_before")
            .is_none()
    );
    input.context_before = 2;
    input.context_after = 2;
    let contextual = tool.grep(input, &cancel).await.unwrap();
    assert_eq!(contextual, original);
}

#[tokio::test]
async fn byte_budget_pages_whole_matches_with_all_requested_context() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a"), "before\nhit\nafter\nhit\nlast\n").unwrap();
    let cancel = CancellationToken::new();
    let mut input = GrepInput::new("hit");
    input.context_before = 1;
    input.context_after = 1;
    input.limit = NonZeroUsize::new(1);
    let unlimited = GrepTool::new(root.path()).await.unwrap();
    let first = unlimited.grep(input.clone(), &cancel).await.unwrap();
    let limits = GrepLimits {
        max_output_bytes: first.text.len(),
        ..GrepLimits::default()
    };
    let tool = unlimited.with_limits(limits).unwrap();
    input.limit = None;
    let bounded = tool.grep(input.clone(), &cancel).await.unwrap();
    assert_eq!(bounded, first);
    input.offset = NonZeroUsize::new(bounded.data.next_offset.unwrap()).unwrap();
    let last = tool.grep(input, &cancel).await.unwrap();
    assert_eq!(last.data.matches[0].line, 4);
    assert_eq!(last.data.matches[0].context_before[0].text, "after");
    assert_eq!(last.data.matches[0].context_after[0].text, "last");
    assert!(!last.data.truncated);
}

#[tokio::test]
async fn oversized_context_is_omitted_with_an_explicit_clipped_flag() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a"), "very long context\nhit\n").unwrap();
    let cancel = CancellationToken::new();
    let limits = GrepLimits {
        max_output_bytes: 12,
        ..GrepLimits::default()
    };
    let tool = GrepTool::new(root.path())
        .await
        .unwrap()
        .with_limits(limits)
        .unwrap();
    let mut input = GrepInput::new("hit");
    assert_eq!(
        tool.grep(input.clone(), &cancel)
            .await
            .unwrap()
            .data
            .matches
            .len(),
        1
    );
    input.context_before = 1;
    let clipped = tool.grep(input, &cancel).await.unwrap();
    assert_eq!(clipped.text, "\"a\":2: hit\n");
    let found = &clipped.data.matches[0];
    assert!(found.clipped);
    assert_eq!(found.text, "hit");
    assert_eq!(found.context_before, []);
}

#[tokio::test]
async fn the_host_can_bound_or_disable_context_and_invalid_json_is_refused() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a"), "hit\n").unwrap();
    let limits = GrepLimits {
        max_context_lines: 0,
        ..GrepLimits::default()
    };
    let tool = GrepTool::new(root.path())
        .await
        .unwrap()
        .with_limits(limits)
        .unwrap();
    let cancel = CancellationToken::new();
    let mut input = GrepInput::new("hit");
    assert!(tool.grep(input.clone(), &cancel).await.is_ok());
    input.context_after = 1;
    assert!(matches!(
        tool.grep(input, &cancel).await,
        Err(ToolError::LimitExceeded(_))
    ));
    for arguments in [
        serde_json::json!({"pattern":"hit", "context_before":-1}),
        serde_json::json!({"pattern":"hit", "context_after":1.5}),
        serde_json::json!({"pattern":"hit", "context_after":true}),
    ] {
        assert!(serde_json::from_value::<GrepInput>(arguments).is_err());
    }
}
