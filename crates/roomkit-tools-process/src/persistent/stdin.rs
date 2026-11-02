//! Writing to a job's stdin, with bounded waiting and explicit partial effects.

use std::time::Duration;

use roomkit_tools_core::{Output, ToolError};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

use super::{Entry, ProcessSession};
use crate::{ProcessWriteInput, ProcessWriteResult};

impl ProcessSession {
    /// Write exact stdin bytes, with bounded waiting and explicit partial effects.
    ///
    /// # Errors
    /// Rejects unknown IDs, oversized input and pre-call cancellation. Once a write
    /// starts, failure is a structured result; dropped futures may lose that result.
    pub async fn write(
        &self,
        owner: &str,
        input: ProcessWriteInput,
        cancel: &CancellationToken,
    ) -> Result<Output<ProcessWriteResult>, ToolError> {
        if input.text.len() > self.inner.limits.max_write_bytes {
            return Err(ToolError::LimitExceeded(
                "process stdin exceeds host limit".into(),
            ));
        }
        let entry = self.entry(owner, &input.process_id, cancel).await?;
        let deadline =
            tokio::time::Instant::now() + Duration::from_millis(self.inner.limits.write_timeout_ms);
        let mut data = ProcessWriteResult {
            process_id: input.process_id,
            bytes_written: 0,
            complete: false,
            stdin_closed: false,
            error: None,
        };
        let outcome = tokio::select! {
            biased;
            () = cancel.cancelled() => Err("stdin call cancelled"),
            () = entry.stop.cancelled() => Err("process stopped or finished"),
            () = tokio::time::sleep_until(deadline) => Err("stdin write timed out"),
            result = send_input(&entry, &input.text, input.close_stdin, &mut data) => result,
        };
        match outcome {
            Ok(()) => data.complete = true,
            Err(message) => data.error = Some(message.into()),
        }
        data.stdin_closed = entry.record.lock().await.status.stdin_closed;
        let text = format!(
            "{}: bytes_written={}, complete={}, stdin_closed={}, error={:?}\n",
            data.process_id, data.bytes_written, data.complete, data.stdin_closed, data.error
        );
        Ok(Output::new(data, text))
    }
}

async fn send_input(
    entry: &Entry,
    text: &str,
    close: bool,
    data: &mut ProcessWriteResult,
) -> Result<(), &'static str> {
    let mut guard = entry.stdin.lock().await;
    let Some(stdin) = guard.as_mut() else {
        return Err("stdin is closed");
    };
    while data.bytes_written < text.len() {
        let bytes = text
            .as_bytes()
            .get(data.bytes_written..)
            .ok_or("invalid stdin offset")?;
        match stdin.write(bytes).await {
            Ok(0) | Err(_) => {
                let mut record = entry.record.lock().await;
                guard.take();
                record.status.stdin_closed = true;
                return Err("stdin pipe write failed");
            }
            Ok(count) => data.bytes_written += count,
        }
    }
    if close {
        let mut record = entry.record.lock().await;
        guard.take();
        record.status.stdin_closed = true;
    }
    Ok(())
}
