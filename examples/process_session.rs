//! Send an order to a persistent shell and read its response without a model.
#![forbid(unsafe_code)]
#![expect(
    clippy::print_stdout,
    reason = "the example prints its verified result"
)]

use std::error::Error;
use std::time::Duration;

use roomkit_tools_process::{
    ProcessReadInput, ProcessSession, ProcessStartInput, ProcessState, ProcessStatusInput,
    ProcessStopInput, ProcessWriteInput,
};
use tokio_util::sync::CancellationToken;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let root = tempfile::tempdir()?;
    let manager = ProcessSession::new(root.path()).await?;
    let cancel = CancellationToken::new();
    let started = manager
        .start(
            Some("example-demo"),
            ProcessStartInput::new(
                "while IFS= read -r order; do printf 'Received: %s\n' \"$order\"; done",
            ),
            &cancel,
        )
        .await?;
    let id = started.data.process_id;
    let sent = manager
        .write(
            Some("example-demo"),
            ProcessWriteInput {
                text: "43: 2 notebooks\n".into(),
                close_stdin: true,
                ..ProcessWriteInput::new(id.clone())
            },
            &cancel,
        )
        .await?;
    if !sent.data.complete {
        return Err("order was not sent completely".into());
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let states = manager
                .status(
                    Some("example-demo"),
                    ProcessStatusInput {
                        process_id: Some(id.clone()),
                    },
                    &cancel,
                )
                .await?;
            if states
                .data
                .processes
                .first()
                .is_some_and(|s| s.state == ProcessState::Finished)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
        Ok::<(), Box<dyn Error>>(())
    })
    .await??;
    let read = manager
        .read(Some("example-demo"), ProcessReadInput::new(&id), &cancel)
        .await?;
    if read.data.stdout.text != "Received: 43: 2 notebooks\n"
        || read.data.status.exit_code != Some(0)
    {
        return Err("unexpected process result".into());
    }
    println!("{}", read.text);
    let stopped = manager
        .stop(
            Some("example-demo"),
            ProcessStopInput {
                forget: true,
                ..ProcessStopInput::new(id)
            },
            &cancel,
        )
        .await?;
    if !stopped.data.forgotten {
        return Err("process record was not released".into());
    }
    manager.close().await;
    Ok(())
}
