# roomkit-tools-fs

Filesystem tools for agents, each selected and configured on its own: `read`,
`list`, `glob`, `grep`, `write`, `edit`, `diff` and `patch`. Every path is
resolved beneath a host-chosen workspace root and reported relative to it;
pages, revisions and truncation are explicit in the result. Root checks
assume a host-controlled workspace: they are not an OS sandbox.

## Features

None.

## Example

```rust
use roomkit_tools_fs::{ReadInput, ReadTool};
use tokio_util::sync::CancellationToken;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    std::fs::write(root.path().join("hours.txt"), "Monday: 9-17
Saturday: 10-14
")?;

    let read = ReadTool::new(root.path()).await?;
    let page = read
        .read(ReadInput::new("hours.txt"), &CancellationToken::new())
        .await?;
    assert_eq!(page.data.lines.len(), 2);
    assert_eq!(page.data.next_offset, None); // the whole file fitted
    print!("{}", page.text); // numbered lines, for the model
    Ok(())
}
```

## License

MIT. Part of [roomkit-tools](https://github.com/roomkit-live/roomkit-tools),
agent tools a host selects one by one, also available in Python. The
[Rust guide](https://github.com/roomkit-live/roomkit-tools/blob/main/guide/rust.md)
describes every crate.
