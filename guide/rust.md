# Rust integration

Tools are ordinary Rust libraries. Select individual crates without depending on
Python, RoomKit or a model provider. Each implementation derives schemas from its
actual input and output types and returns typed errors and structured output.

## Add a crate

The library crates are published to crates.io as pre-releases. A plain
`"0.1"` requirement does not match a pre-release, so give the exact version:

```toml
[dependencies]
roomkit-tools-fs = "0.1.0-alpha.16"
tokio = { version = "1", features = ["macros", "rt"] }
tokio-util = { version = "0.7", features = ["rt"] }
```

For unreleased changes, depend on a checkout next to your application,
`roomkit-tools-fs = { path = "../roomkit-tools/crates/roomkit-tools-fs" }`, or
on a Git revision, which selects a crate by package name; pin the exact
revision you intend to consume:

```toml
[dependencies]
roomkit-tools-fs = { git = "https://github.com/roomkit-live/roomkit-tools", rev = "<commit>" }
```

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

The limits and configuration structs stand for Python's constructor keywords:
set the fields you need and take the rest from their defaults, as in
`ReadLimits { max_lines: 200, ..ReadLimits::default() }`.

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
| `roomkit-tools-http` | Internal: the DNS-pinned transport, origin policy and body limits the web and search crates share |
| `roomkit-tools-web` | Fetching HTTP text and HTML conversion |
| `roomkit-tools-search` | Search contracts and optional service backends |
| `roomkit-tools-docs` | Documentation contracts, injected backend and optional `files` snapshot |
| `roomkit-tools-plans` | Plan CRUD contracts, injected storage and bounded memory backend |
| `roomkit-tools-agents` | Agent contracts and facades over an injected `AgentBackend` |

Search features `exa`, `ollama` and `openai` are off by default. Enable only the
backends needed by the host. Agent facades own no scheduler: their backend owns
execution, state and resource lifetimes. There is no global tool registry.

## Optional RoomKit Rust adapter

`adapters/roomkit` is a separate workspace. It pins `roomkit` and `roomkit-ai`
from the `roomkit-rs` repository at an immutable revision; the source is not
required by standalone tools or the Python package. The `roomkit-rs` repository
is not public yet, so building the adapter requires access to it. Consume the
same source and revision to share RoomKit types.

The adapter is not published to crates.io, which refuses its Git dependencies.
Depend on it through Git, and take the tool crates it wraps from the same
revision: a crate from crates.io and the same crate from Git are distinct
crates, whose types do not match.

```toml
[dependencies]
roomkit-tools-roomkit = { git = "https://github.com/roomkit-live/roomkit-tools", rev = "<commit>" }
roomkit-tools-fs = { git = "https://github.com/roomkit-live/roomkit-tools", rev = "<commit>" }
```

Each adapter implements RoomKit's `Tool` trait, the Rust form of Python's Tool
protocol, for filesystem, outline, command, process, output, web-fetch and
search tools. Declare them individually in an AI channel's
`AiChannelOptions::tools` as `ToolEntry::Tool(Arc::new(adapter))`, and supply
roots, limits and lifetimes. An adapter serves only its own tool's name and
leaves any other call to the channel's next tool. RoomKit manages argument
validation, policies, hooks and turn cancellation; dropping a call cancels it.
Process/archive owners are derived from the conversation and agent of the
current RoomKit tool call, and a call made outside one is refused:
`roomkit_owner(room_id, channel_id)` returns that key, so the host can pass it
to `ProcessSession` and `OutputStore` methods to reach the jobs and archives of
an agent's calls in a room. Their `owner` is an `Option<&str>`: `None` is the
standalone owner, as Python's `owner=None`, apart from every named owner,
`"default"` included; `close_owner(None)` closes its jobs.

| Capability | Native Rust tool | RoomKit Rust adapter |
| --- | --- | --- |
| Files, outlines, shell/processes, output, web/search | Implemented | Implemented |
| `view_image` | Implemented | Pending: pinned engine tool output supports text/JSON only |
| Documentation | Implemented, file backend with `files` feature | Implemented with optional `docs` feature |
| Plan CRUD | Implemented | Implemented with optional `plans` feature |
| Agent tools | Contracts and injected backend interface | Native RoomKit execution backend not implemented |

RoomKit Python already supports image results and has a packaged agent backend.
These differences are integration boundaries, not separate tool implementations.
See [images](images.md), [plans](plans.md) and [agents](agents.md).

## Lifetimes and cancellation

Pass a `CancellationToken` to operations. A returned cancellation does not undo
an already-committed mutation or immediately terminate blocking work. Keep the
Tokio runtime alive while cleanup completes. Use explicit process/store/backend
shutdown: the adapters close nothing, and the host closes the `ProcessSession`,
`OutputStore` and backends it shares between them. State is not durable across
a host restart.

Filesystem roots are access checks for a host-controlled workspace. Shell
commands execute with host permissions and only the environment the host selects
with `CommandEnv`; the initial directory is not a sandbox.
See the tool-family guides for exact resource and cancellation boundaries.

[Documentation index](README.md) · [Project overview](../README.md)

OCR uses the separate `roomkit-tools-ocr` contract and optional
`roomkit-tools-ocr-rusto` engine. Enable the adapter's `ocr` feature for
`Ocr`. See [OCR](ocr.md) for model configuration and native-worker ownership.
