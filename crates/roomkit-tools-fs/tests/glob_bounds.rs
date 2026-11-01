//! Glob stays cancellable under costly patterns.

use std::path::Path;
use std::time::{Duration, Instant};

use roomkit_tools_core::Error;
use roomkit_tools_fs::{GlobInput, GlobTool};
use tokio_util::sync::CancellationToken;

/// A pattern that must track every `a` among a name's last 61 characters:
/// no small automaton does that, so matching each name is slow.
fn costly_pattern() -> String {
    format!("*a{}", "?".repeat(60))
}

/// Names of 200 bytes ending in pseudo-random `a` and `b` letters.
#[expect(clippy::unwrap_used, reason = "a fixture must be created successfully")]
fn costly_names(root: &Path, count: usize) {
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    for index in 0..count {
        let letters: String = (0..195)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                if state & 1 == 0 { 'a' } else { 'b' }
            })
            .collect();
        std::fs::write(root.join(format!("{index:05}{letters}")), "").unwrap();
    }
}

#[test]
fn cancellation_returns_promptly_and_stops_the_matching_worker() {
    let root = tempfile::tempdir().unwrap();
    costly_names(root.path(), 4_000);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let returned = runtime.block_on(async {
        let tool = GlobTool::new(root.path()).await.unwrap();
        let cancel = CancellationToken::new();
        let stop = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            stop.cancel();
        });
        let start = Instant::now();
        let result = tool.glob(GlobInput::new(costly_pattern()), &cancel).await;
        assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
        start.elapsed()
    });
    assert!(
        returned < Duration::from_millis(500),
        "returned after {returned:?}"
    );
    // Dropping the runtime waits for blocking work: the worker must stop at
    // its next entry instead of matching the remaining names.
    let start = Instant::now();
    drop(runtime);
    let stopped = start.elapsed();
    assert!(
        stopped < Duration::from_secs(2),
        "worker ran for {stopped:?}"
    );
}
