//! A read's wait for output, and the text a model reads of the read.

use std::fmt::Write;
use std::time::Duration;

use roomkit_tools_core::ToolError;
use tokio_util::sync::CancellationToken;

use super::registry::Entry;
use crate::{ProcessReadInput, ProcessReadResult};

/// What a model reads of a read: the status line, the archives, then each
/// stream's page.
pub(super) fn read_text(data: &ProcessReadResult) -> String {
    let mut text = super::status_line(&data.status);
    text.push('\n');
    // Before what the command wrote, which could imitate these lines.
    crate::output::append_metadata(
        &mut text,
        data.status.stdout_archive.as_ref(),
        data.status.stderr_archive.as_ref(),
    );
    for (name, page) in [("stdout", &data.stdout), ("stderr", &data.stderr)] {
        let _written = write!(text, "{name} (next_offset={}", page.next_offset);
        if page.omitted_bytes > 0 {
            let _written = write!(text, ", omitted_bytes={}", page.omitted_bytes);
        }
        if page.has_more {
            text.push_str(", has_more=true");
        }
        text.push_str("):\n");
        if !page.text.is_empty() {
            text.push_str(&page.text);
            if !page.text.ends_with('\n') {
                text.push('\n');
            }
        }
    }
    text
}

/// Wait until `entry` finishes or writes past `input`'s offsets, or `wait`
/// elapses; a cancelled call stops waiting as cancelled.
pub(super) async fn wait_for_output(
    entry: &Entry,
    input: &ProcessReadInput,
    wait: Duration,
    cancel: &CancellationToken,
) -> Result<(), ToolError> {
    let deadline = tokio::time::Instant::now() + wait;
    let mut done = entry.done.clone();
    // Subscribed before the first look, so no write between them is missed.
    let mut output = entry.record.lock().await.output.subscribe();
    loop {
        if *done.borrow_and_update() || entry.record.lock().await.ready_for(input) {
            return Ok(());
        }
        tokio::select! {
            () = cancel.cancelled() => return Err(ToolError::Cancelled),
            () = tokio::time::sleep_until(deadline) => return Ok(()),
            changed = done.changed() => if changed.is_err() { return Ok(()) },
            changed = output.changed() => if changed.is_err() { return Ok(()) },
        }
    }
}
