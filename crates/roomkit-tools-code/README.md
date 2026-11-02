# roomkit-tools-code

The `code_outline` tool: a bounded outline of the classes, functions and
methods of a Rust or Python file, with line ranges an agent can then read.
Tree-sitter parses the source without executing it; this is not type
resolution, macro expansion or an LSP server.

## Features

None. The Rust and Python grammars are always included.

## Example

```rust
use roomkit_tools_code::{CodeOutlineInput, CodeOutlineTool};
use tokio_util::sync::CancellationToken;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    std::fs::write(
        root.path().join("shipping.py"),
        "class Shipping:\n    def fee(self, subtotal):\n        return 0 if subtotal >= 50 else 5\n",
    )?;

    let tool = CodeOutlineTool::new(root.path()).await?;
    let outline = tool
        .outline(CodeOutlineInput::new("shipping.py"), &CancellationToken::new())
        .await?;
    let names: Vec<&str> = outline.data.symbols.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["Shipping", "fee"]);
    Ok(())
}
```

## License

MIT. Part of [roomkit-tools](https://github.com/roomkit-live/roomkit-tools),
agent tools a host selects one by one, also available in Python. The
[Rust guide](https://github.com/roomkit-live/roomkit-tools/blob/main/guide/rust.md)
describes every crate.
