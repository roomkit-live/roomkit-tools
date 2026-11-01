//! Grep stays cancellable under costly regexes and always makes progress.

use std::num::NonZeroUsize;
use std::time::{Duration, Instant};

use roomkit_tools_core::Error;
use roomkit_tools_fs::{GrepInput, GrepLimits, GrepTool};
use tokio_util::sync::CancellationToken;

/// A regex whose linear-time search is still slow on long lines of letters
/// broken by a non-letter just before the required 4,000-letter run.
const COSTLY: &str = "[a-z]*[aeiou][a-z]{4000}[^a-z]";

fn costly_line(bytes: usize) -> String {
    let run: String = ('a'..='z').cycle().take(3999).collect();
    format!("{run}!").repeat(bytes / 4000)
}

#[test]
fn cancellation_returns_promptly_and_stops_the_search_worker() {
    let root = tempfile::tempdir().unwrap();
    let line = costly_line(16_000);
    std::fs::write(
        root.path().join("costly.txt"),
        format!("{line}\n").repeat(16),
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let returned = runtime.block_on(async {
        let tool = GrepTool::new(root.path()).await.unwrap();
        let cancel = CancellationToken::new();
        let stop = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            stop.cancel();
        });
        let start = Instant::now();
        let result = tool.grep(GrepInput::new(COSTLY), &cancel).await;
        assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
        start.elapsed()
    });
    assert!(
        returned < Duration::from_millis(500),
        "returned after {returned:?}"
    );
    // Dropping the runtime waits for blocking work: the worker must stop at
    // its next line instead of scanning the remaining lines.
    let start = Instant::now();
    drop(runtime);
    let stopped = start.elapsed();
    assert!(
        stopped < Duration::from_secs(2),
        "worker ran for {stopped:?}"
    );
}

#[tokio::test]
async fn lines_longer_than_the_host_bound_are_skipped_and_counted() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("notes.txt"),
        "short hit\nthis line is far too long to search for hit\nhit\n",
    )
    .unwrap();
    let mut limits = GrepLimits::default();
    limits.max_line_bytes = 16;
    let tool = GrepTool::new(root.path())
        .await
        .unwrap()
        .with_limits(limits)
        .unwrap();
    let page = tool
        .grep(GrepInput::new("hit"), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        page.data.matches.iter().map(|m| m.line).collect::<Vec<_>>(),
        [1, 3]
    );
    assert_eq!(page.data.skipped_lines, 1);
    limits.max_line_bytes = 0;
    assert!(matches!(
        GrepTool::new(root.path())
            .await
            .unwrap()
            .with_limits(limits),
        Err(Error::InvalidInput(_))
    ));
}

#[tokio::test]
async fn an_oversized_first_match_is_clipped_so_paging_moves_past_it() {
    let root = tempfile::tempdir().unwrap();
    let long = format!("needle {}", "π".repeat(28_000));
    std::fs::write(
        root.path().join("data.txt"),
        format!("{long}\nneedle short\n"),
    )
    .unwrap();
    let tool = GrepTool::new(root.path()).await.unwrap();
    let cancel = CancellationToken::new();
    let mut input = GrepInput::new("needle");
    input.context_after = 1;
    let first = tool.grep(input.clone(), &cancel).await.unwrap();
    let clipped = &first.data.matches[0];
    assert_eq!(first.data.matches.len(), 1);
    assert!(clipped.clipped);
    assert_eq!(clipped.line, 1);
    assert_eq!(clipped.context_after, []);
    assert!(long.starts_with(&clipped.text));
    assert!(clipped.text.len() < long.len());
    assert!(first.text.len() <= GrepLimits::default().max_output_bytes);
    assert_eq!(first.data.next_offset, Some(2));
    input.offset = NonZeroUsize::new(2).unwrap();
    let second = tool.grep(input, &cancel).await.unwrap();
    assert_eq!(second.data.matches[0].text, "needle short");
    assert!(!second.data.matches[0].clipped);
    let wire = serde_json::to_value(&second).unwrap();
    assert!(wire["data"]["matches"][0].get("clipped").is_none());
}
