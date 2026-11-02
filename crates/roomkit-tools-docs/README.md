# roomkit-tools-docs

Documentation tools over a backend the host injects: `docs_list` discovers
documents, `docs_search` finds literal terms, and `docs_read` returns pages of
complete source lines. Every result carries the document and catalogue
revisions, so an agent can cite what it read and detect a change.

## Features

- `files` (off by default): `FileDocs`, a bounded snapshot of a directory of
  Markdown and text files.

## Example

With `features = ["files"]`:

```rust
use std::sync::Arc;

use roomkit_tools_docs::{DocsBackend, DocsSearchInput, DocsSearchTool, FileDocs, FileDocsLimits};
use tokio_util::sync::CancellationToken;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    std::fs::write(root.path().join("shipping.md"), "# Shipping\n\nOrders ship free from 50 units.\n")?;

    let docs: Arc<dyn DocsBackend> = Arc::new(FileDocs::new(root.path(), FileDocsLimits::default())?);
    let found = DocsSearchTool::new(docs)
        .call(DocsSearchInput::new("free units"), &CancellationToken::new())
        .await?;
    assert_eq!(found.data.matched_documents, 1);
    print!("{}", found.text);
    Ok(())
}
```

## License

MIT. Part of [roomkit-tools](https://github.com/roomkit-live/roomkit-tools),
agent tools a host selects one by one, also available in Python. The
[Rust guide](https://github.com/roomkit-live/roomkit-tools/blob/main/guide/rust.md)
describes every crate.
