# Configured documentation

Give an agent a selected reference library with `docs_list`, `docs_search` and
`docs_read`. Each tool is optional. Documentation can describe workflows, APIs or
how an application operates; it does not establish which tools the host has
actually enabled, grant permissions, or execute the documented actions.

The current backend reads a configured directory of Markdown and text files.
HTTPS is a future backend, not an implemented option. The common contracts allow
other host-provided backends without changing the model-facing tools.

## Python and RoomKit

```python
from roomkit import AIChannel
from roomkit_tools import DocsList, DocsRead, DocsSearch, FileDocs

documentation = FileDocs("./reference", version="application-v1")
agent = AIChannel(
    "assistant",
    provider=provider,  # Your configured RoomKit provider.
    tools=[DocsList(documentation), DocsSearch(documentation), DocsRead(documentation)],
)
```

The host chooses the root, version and tools. A call cannot change the root or
fetch an arbitrary path/URL. Share a backend to share its snapshot; use different
instances for different reference libraries or access scopes. There is no global
catalogue and no automatic injection into every agent.

Without RoomKit, call the selected tool directly:

```python
search = DocsSearch(documentation)
result = await search.call({"query": "agent_followup idle"})
if result["data"]["matches"]:
    doc = result["data"]["matches"][0]["document"]
    page = await DocsRead(documentation).call({
        "document_id": doc["document_id"],
        "expected_revision": doc["revision"],
        "offset": 1,
        "limit": 50,
    })
```

`FileDocs.list/search/read` expose the same operations as data dictionaries;
the tool facades add the standard `{data, text}` envelope. For these tools, `text`
is a JSON representation of `data`, preserving the complete provenance and lines.
RoomKit also receives the envelope as structured tool content.

## Discovery, search and reading

| Tool | Inputs | Result |
| --- | --- | --- |
| `docs_list` | `offset=0`, `limit=20`, optional `expected_catalog_revision` | Document IDs, titles, sources, versions, revisions and line counts |
| `docs_search` | `query`, `offset=0`, `limit=10`, optional `expected_catalog_revision` | One ranked excerpt per matching document, with metadata and original line number |
| `docs_read` | `document_id`, `offset=1`, `limit=100`, optional `expected_revision` | Complete numbered source lines and document metadata |

List/search offsets count results from zero; read offsets count lines from one.
Follow `next_offset` until null. `truncated` means more results/lines remain.
Offsets immediately after the last result/line produce an empty final page;
larger offsets are rejected. Search hits have their own excerpt `truncated` and
`omitted_bytes` fields, distinct from result pagination.

Search uses literal, case-insensitive substring matching. All whitespace-separated
terms must occur somewhere in the document's title, ID or body. Terms can occur
on different lines. There are no regex, query operators, embeddings or hidden web
fallbacks. For each term, a title match scores 8, otherwise an ID match scores 4,
otherwise a body match scores 1. Ties sort by document ID. The excerpt is the first
source line containing any term, or the title with `line=null` for metadata-only
matches. Read the document for full context; the excerpt need not contain all terms.

A document revision is `sha256:` plus the original bytes' lowercase SHA-256.
The catalogue revision covers the ordered metadata and host version label.
Expected revisions are optional guards: a mismatch is refused. Copy returned
revisions rather than reconstructing them. `version` is a host-declared label,
not a detected package/runtime version or proof of available capabilities.

## File snapshot and limits

`FileDocs` captures a relative root against the construction-time working
directory, then loads a bounded immutable snapshot on the first successful call.
Clones/tools share it. File edits are not watched: construct a new backend to
refresh. Failed or cancelled initialization can be retried. Files are read
sequentially, so initialization is not an atomic snapshot across concurrent
external file edits. Use a stable, host-controlled directory.

Eligible files are UTF-8 `.md` and `.txt` (case-insensitive extensions), including
nested directories. Hidden names and symlinks are excluded. Other extensions are
ignored. Eligible non-regular files, invalid UTF-8 or NUL bytes fail the load.
IDs and sources are relative paths; titles use the first `# ` heading, or the ID.
Titles are clipped at 256 UTF-8 bytes. A UTF-8 BOM is removed for display and CRLF
is normalized into logical lines; revisions still hash the original bytes.

| Host setting | Default | Allowed range |
| --- | --- | --- |
| `max_documents` | 256 | 1–1024 |
| `max_document_bytes` | 256 KiB | 1 byte–1 MiB |
| `max_total_bytes` | 8 MiB | 1 byte–64 MiB |
| `max_entries` | 4096 | 1–65536 scanned entries, including ignored entries |
| `max_depth` | 16 | 1–64 nested directory levels |
| `max_total_lines` | 65536 | 1–131072 logical lines |
| `max_read_bytes` | 32 KiB | 4096–65536 bytes of serialized page budget |

Python accepts these as `FileDocs` keyword arguments; Rust uses `FileDocsLimits`.
Exceeding a scan/load limit fails the whole catalogue instead of returning a
silently incomplete search space. These bound work and content, not exact peak
memory: the snapshot also stores lines, metadata and lowercase search copies.

List pages allow 1–100 documents, search pages 1–50, and read pages 1–500 lines.
Queries allow 16 whitespace-separated terms and 1024 UTF-8 bytes. Search excerpts
are clipped at 1024 UTF-8 bytes without splitting characters. Reads stop between
lines to respect the page budget; a single line that cannot fit is refused.
Facades reject serialized result data above 128 KiB; request a smaller page if
large metadata makes list/search results exceed this bound.

Blocking file work and search/read processing run outside the async event loop,
with at most two native workers per shared backend. Cancellation is cooperative;
an issued filesystem operation may finish after cancellation returns. Its worker
retains its slot until it finishes. Keep the runtime alive for cleanup. The root
checks assume a host-controlled tree; they are not a hostile-filesystem sandbox.

## Rust and custom backends

Select `roomkit-tools-docs` with feature `files` for `FileDocs`. Default features
are empty: the contracts and `DocsBackend` need neither file I/O nor the SHA-256
implementation dependency. Supply `Arc<dyn DocsBackend>` to `DocsListTool`,
`DocsSearchTool` and `DocsReadTool`, then `call(input, &cancel)`, Python's
`call(arguments)`.

The separate RoomKit Rust adapter's optional `docs` feature exposes
`DocsList`, `DocsSearch` and `DocsRead`. Construct each from its
native facade. Dropping a call cancels it without invalidating the host-owned
backend shared by other tools.

Python `DocsBackend` is a protocol with asynchronous `list`, `search` and `read`
methods returning data dictionaries, not envelopes. The selected facade validates
inputs and outputs using the same Rust-derived schemas; `.contract()` exposes
those schemas. Custom backends implement bounds, the documented search semantics,
provenance, revision checks and cancellation. They must enforce their own access
policy. Tool selection alone does not make arbitrary backend I/O safe.

## Checked examples

```sh
cargo run --example documentation
python examples/roomkit_docs.py
python examples/roomkit_docs.py --live
python examples/roomkit_docs.py --chat
```

The scripted RoomKit example needs no server. Live modes use `OPENAI_BASE_URL`,
`OPENAI_MODEL` and `OPENAI_API_KEY`. One initial request checks discovery, searches
and revision-checked reads of two generic guides; `--chat` then accepts ordinary
questions until `quit`. Only documentation tools are enabled, even though the
sample guides describe planning and delegation.

[Documentation index](README.md) · [Examples](../examples/README.md)
