# Rust integration

Tools are ordinary Rust libraries. Select individual crates without depending on
Python, RoomKit or a model provider. Each implementation derives schemas from its
actual input and output types and returns typed errors and structured output.

## Add a crate

Crates are not published to crates.io. With this repository checked out next to
your application, add a path dependency:

```toml
[dependencies]
roomkit-tools-fs = { path = "../roomkit-tools/crates/roomkit-tools-fs" }
tokio = { version = "1", features = ["macros", "rt"] }
tokio-util = { version = "0.7", features = ["rt"] }
```

A Git dependency can select a crate by package name; pin the exact revision you
intend to consume. Git access is required while the repository is private.
The minimum supported Rust version is 1.94.

For a file beneath the current directory:

```rust
use roomkit_tools_fs::{ReadInput, ReadTool};
use tokio_util::sync::CancellationToken;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let read = ReadTool::new(".").await?;
    let result = read
        .read(ReadInput::new("README.md"), &CancellationToken::new())
        .await?;
    print!("{}", result.text);
    Ok(())
}
```

Within this repository, `cargo run --example read_file` runs a complete
self-contained check. More examples are listed in the [example index](../examples/README.md).
Run `cargo doc --workspace --no-deps --open` for field-level API documentation.

## Crate boundaries

| Crate | Responsibility |
| --- | --- |
| `roomkit-tools-core` | Definitions, schemas, structured output and errors; no I/O or async runtime |
| `roomkit-tools-fs` | Text files, discovery, revisions, mutations, comparisons and structured patches |
| `roomkit-tools-process` | Shell, persistent jobs, output stores, owner scopes and cleanup |
| `roomkit-tools-code` | Tree-sitter outlines for Rust and Python |
| `roomkit-tools-image` | Local image decoding and bounded PNG previews |
| `roomkit-tools-http` | Shared DNS-pinned transport, origin policy and body limits |
| `roomkit-tools-web` | Fetching HTTP text and HTML conversion |
| `roomkit-tools-search` | Search contracts and optional service backends |
| `roomkit-tools-agents` | Agent contracts and facades over an injected `AgentBackend` |

Search features `exa`, `ollama` and `openai` are off by default. Enable only the
backends needed by the host. Agent facades own no scheduler: their backend owns
execution, state and resource lifetimes. There is no global tool registry.

## Optional RoomKit Rust adapter

`adapters/roomkit` is a separate workspace. It pins `roomkit` and `roomkit-ai`
from the `roomkit-rs` repository at an immutable revision; the source is not
required by standalone tools or the Python package. Fetching that engine may
require Git access. Consume the same source and revision to share RoomKit types.

The adapter implements RoomKit's `Toolset` interface for filesystem, outline,
command, process, output, web-fetch and search tools. Register them individually
and supply roots, limits and lifetimes. RoomKit manages argument validation,
policies, hooks and turn cancellation. Process/archive owners are derived from
the conversation and agent.

| Capability | Native Rust tool | RoomKit Rust adapter |
| --- | --- | --- |
| Files, outlines, shell/processes, output, web/search | Implemented | Implemented |
| `view_image` | Implemented | Pending: pinned engine tool output supports text/JSON only |
| Agent tools | Contracts and injected backend interface | Native RoomKit execution backend not implemented |

RoomKit Python already supports image results and has a packaged agent backend.
These differences are integration boundaries, not separate tool implementations.
See [images](images.md) and [agents](agents.md).

## Lifetimes and cancellation

Pass a `CancellationToken` to operations. A returned cancellation does not undo
an already-committed mutation or immediately terminate blocking work. Keep the
Tokio runtime alive while cleanup completes. Use explicit process/store/backend
shutdown; state is not durable across a host restart.

Filesystem roots are access checks for a host-controlled workspace. Shell
commands execute with host permissions; the initial directory is not a sandbox.
See the tool-family guides for exact resource and cancellation boundaries.

[Documentation index](README.md) · [Project overview](../README.md)

OCR uses the separate `roomkit-tools-ocr` contract and optional
`roomkit-tools-ocr-rusto` engine. Enable the adapter's `ocr` feature for
`OcrToolset`. See [OCR](ocr.md) for model configuration and native-worker ownership.
