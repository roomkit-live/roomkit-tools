//! Grep stays cancellable under costly regexes and always makes progress.

use std::num::NonZeroUsize;
use std::time::{Duration, Instant};

use roomkit_tools_core::ToolError;
use roomkit_tools_fs::{GrepInput, GrepLimits, GrepTool};
use tokio_util::sync::CancellationToken;

/// A regex whose linear-time search is still slow on long lines of letters
/// broken by a non-letter just before the required 4,000-letter run.
const COSTLY: &str = "[a-z]*[aeiou][a-z]{4000}[^a-z]";

fn costly_line(bytes: usize) -> String {
    let run: String = ('a'..='z').cycle().take(3999).collect();
    format!("{run}!").repeat(bytes / 4000)
}

#[expect(clippy::unwrap_used, reason = "a test runtime must start")]
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
}

/// How long this machine takes to search one costly line, so the worker's
/// bound scales with the machine instead of assuming its speed.
#[expect(clippy::unwrap_used, reason = "the calibration search must succeed")]
fn one_line_search(line: &str) -> Duration {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("one.txt"), format!("{line}\n")).unwrap();
    runtime().block_on(async {
        let tool = GrepTool::new(root.path()).await.unwrap();
        let start = Instant::now();
        tool.grep(GrepInput::new(COSTLY), &CancellationToken::new())
            .await
            .unwrap();
        start.elapsed()
    })
}

#[test]
fn cancellation_returns_promptly_and_stops_the_search_worker() {
    let root = tempfile::tempdir().unwrap();
    let line = costly_line(16_000);
    let one_line = one_line_search(&line);
    std::fs::write(
        root.path().join("costly.txt"),
        format!("{line}\n").repeat(32),
    )
    .unwrap();
    let runtime = runtime();
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
        assert!(matches!(result, Err(ToolError::Cancelled)), "{result:?}");
        start.elapsed()
    });
    assert!(
        returned < Duration::from_millis(500),
        "returned after {returned:?}"
    );
    // Dropping the runtime waits for blocking work: the worker must finish
    // the line it is on and stop, instead of scanning the 31 others.
    let start = Instant::now();
    drop(runtime);
    let stopped = start.elapsed();
    assert!(
        stopped < one_line * 4,
        "worker ran for {stopped:?}, one line takes {one_line:?}"
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
    let mut limits = GrepLimits {
        max_line_bytes: 16,
        ..GrepLimits::default()
    };
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
        Err(ToolError::InvalidInput(_))
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
