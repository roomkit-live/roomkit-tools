//! Blocking workers for filesystem tools, so that blocking calls and costly
//! matching never occupy the async executor.

use roomkit_tools_core::Error;

/// Run `work` on a blocking worker and wait for its result. A worker that
/// fails to finish is reported as I/O with `operation`.
pub(crate) async fn blocking<T, F>(operation: &'static str, work: F) -> Result<T, Error>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, Error> + Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|source| Error::Io {
            operation,
            source: std::io::Error::other(source),
        })?
}
