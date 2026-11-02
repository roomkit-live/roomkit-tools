# roomkit-tools-core

The contracts every roomkit-tools crate shares: a tool's `Definition` (name,
description, and JSON Schemas derived from its Rust input and output types),
the `Output<T>` envelope (structured `data` for the host, `text` for the
model) and `ToolError`, the failures a host classifies without parsing a
message. It performs no I/O and needs no async runtime.

You rarely depend on it alone: the tool crates return its types.

## Features

None.

## Example

```rust
use roomkit_tools_core::{Output, ToolError};

/// The text a host sends back to the model after a tool call.
fn reply<T>(result: Result<Output<T>, ToolError>) -> String {
    match result {
        Ok(output) => output.text,
        // Ask the model to fix its arguments; nothing ran.
        Err(error @ ToolError::InvalidInput(_)) => format!("{error}. Correct the call."),
        // An I/O failure's message names the operation, never the OS error.
        Err(error) => error.to_string(),
    }
}

let refused: Result<Output<()>, ToolError> =
    Err(ToolError::AccessRefused("path is outside the workspace".into()));
assert_eq!(reply(refused), "Access refused: path is outside the workspace");
```

## License

MIT. Part of [roomkit-tools](https://github.com/roomkit-live/roomkit-tools),
agent tools a host selects one by one, also available in Python. The
[Rust guide](https://github.com/roomkit-live/roomkit-tools/blob/main/guide/rust.md)
describes every crate.
