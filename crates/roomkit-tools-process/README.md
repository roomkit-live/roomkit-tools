# roomkit-tools-process

Command execution for agents: the foreground `shell` tool, persistent
processes (`process_start`, `process_status`, `process_read`,
`process_write`, `process_stop`) owned by a `ProcessSession`, and an
`OutputStore` that archives long output for paged reads. Output is bounded,
each command leads its own session and its process group is stopped on
cleanup. Commands run with the host's permissions and only the environment
the host selects with `CommandEnv`: the working directory is not a sandbox.
Unix only; other platforms return an explicit refusal.

## Features

None.

## Example

```rust
use roomkit_tools_process::{ShellInput, ShellTool};
use tokio_util::sync::CancellationToken;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let shell = ShellTool::new(root.path()).await?;
    let result = shell
        .shell(ShellInput::new("echo ready"), &CancellationToken::new())
        .await?;
    // A non-zero exit or a timeout is a result, not an error.
    assert_eq!(result.data.exit_code, Some(0));
    assert_eq!(result.data.stdout.text, "ready\n");
    Ok(())
}
```

## Host observation

Subscribe with `ProcessSession::subscribe()` before calling `observe()`. The watch
receiver coalesces starts, completions, removal and shutdown; output chunks do not
produce notifications. Observations include the exact command, trusted owner key,
elapsed runtime (frozen on completion) and current status. This is a privileged
host API: filter owners before displaying jobs to a user. It does not change the
owner-scoped model tool contracts. Use bounded `read()` pages when a view is open,
and drop the watch receiver when the view disconnects.

## License

MIT. Part of [roomkit-tools](https://github.com/roomkit-live/roomkit-tools),
agent tools a host selects one by one, also available in Python. The
[Rust guide](https://github.com/roomkit-live/roomkit-tools/blob/main/guide/rust.md)
describes every crate.
