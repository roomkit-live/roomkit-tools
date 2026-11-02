//! Blocking workers for filesystem tools, so that blocking calls and costly
//! matching never occupy the async executor, and the cancellation they obey.

use std::future::Future;

use roomkit_tools_core::ToolError;
use tokio_util::sync::CancellationToken;

/// Await `work` unless `cancel` fires first, in which case return
/// [`ToolError::Cancelled`] at once.
///
/// `work` receives a child token that is cancelled when this call returns or
/// is dropped, so a blocking worker it started stops at its next [`check`].
pub(crate) async fn cancellable<T, F, Fut>(
    cancel: &CancellationToken,
    work: F,
) -> Result<T, ToolError>
where
    F: FnOnce(CancellationToken) -> Fut,
    Fut: Future<Output = Result<T, ToolError>>,
{
    let cancel = cancel.child_token();
    let _stop_worker = cancel.clone().drop_guard();
    tokio::select! { biased;
        () = cancel.cancelled() => Err(ToolError::Cancelled),
        result = work(cancel.clone()) => result,
    }
}

/// Run `work` on a blocking worker and wait for its result. A worker that
/// fails to finish is reported as I/O with `operation`.
pub(crate) async fn blocking<T, F>(operation: &'static str, work: F) -> Result<T, ToolError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, ToolError> + Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|source| ToolError::Io {
            operation,
            source: std::io::Error::other(source),
        })?
}

/// Return [`ToolError::Cancelled`] once `cancel` has fired; blocking workers
/// call it between bounded steps.
pub(crate) fn check(cancel: &CancellationToken) -> Result<(), ToolError> {
    if cancel.is_cancelled() {
        Err(ToolError::Cancelled)
    } else {
        Ok(())
    }
}
