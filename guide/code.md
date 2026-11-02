# Source outlines

`roomkit-tools-code` provides `CodeOutlineTool` independently of filesystem,
process, web, Python and RoomKit crates. It uses Tree-sitter with Rust and Python
grammars to describe one UTF-8 file without running or importing its code.

Inside an async function, with an existing workspace:

```python
from roomkit_tools import CodeOutline, CodeOutlineTool

# Select it in a RoomKit agent alongside Read/Edit as needed.
tools = [CodeOutline(root="./workspace")]

# Or use the standalone async API.
outline = await CodeOutlineTool.create("./workspace")
result = await outline.outline("shipping.py")
for symbol in result["data"]["symbols"]:
    print(symbol["name"], symbol["start_line"], symbol["end_line"])
```

Rust hosts use `CodeOutlineTool::new(root).await?` and
`outline(CodeOutlineInput::new(path), &cancel).await?`; the optional Rust adapter
is `CodeOutline`. See `examples/outline_code.rs` for outline-to-read and
`python examples/roomkit_outline.py` for a real scripted repair. Add `--live`
with the [example provider configuration](../examples/README.md#use-a-live-model)
for a model-driven run.

The language is inferred from the resolved file's `.rs`, `.py` or `.pyi`
extension, or selected explicitly with `language="rust"` / `"python"`. Rust
outlines include functions, methods, structs, enums, traits, impls, modules and
type aliases; Python includes classes, methods and nested functions. Signatures
preserve source headers, including Rust attributes and Python decorators. Bodies,
fields, enum variants, imports, assignments and macro expansions are not an
inventory of symbols. No type resolution, compiler checking, import following,
project index or LSP service is implied.

`data.path` names the file relative to the workspace root, whether the request
used a relative or an absolute path. `data.symbols` is flat and ordered by
source position. Each symbol has a one-based
`index`, optional enclosing `parent`, `kind`, `name`, `signature`, and inclusive
`start_line` / `end_line`. Use those lines with `read`; `offset` / `next_offset`
on **outline** count symbols instead. Parent indexes may refer to earlier pages.
Each call reparses the file. Carry `data.revision` as `expected_revision` when
paging to refuse changes; this SHA-256 covers raw bytes, including BOM and CRLF,
and is compatible with the filesystem tools' revisions.

Inspect `syntax_errors` for parser recovery, `signature_truncated` for shortened
headers, and `truncated` / `next_offset` for further symbols. Invalid source can
still yield useful definitions; it is never reported as syntax-error-free.
Limits default to 512 KiB input, 64 KiB of complete JSON data-plus-text output,
200 symbols per page, 5,000 scanned definitions, 200,000 traversed syntax nodes,
depth 256 and 1,024 bytes per signature. Oversized names or scan budgets fail;
signatures clip at UTF-8 boundaries. All bounds are host-configurable.

The default deadline is 10 seconds, capped at 120 seconds by default. Queuing,
I/O, parsing and rendering share that deadline. Two worker slots are shared by
native tool clones; blocking workers retain their slot until cooperative
cancellation finishes. Whole-file parsing retains source bytes and a syntax
tree; these limits are not a hard memory sandbox. Tree-sitter builds the whole
tree before the node and depth bounds can apply, so parser memory follows the
input size and shape. Measured peak growth per worker slot (VmHWM, Linux x86-64
release build):

| Input | Peak |
| --- | --- |
| Real 229 KB Python file / 184 KB Rust file | about 5 MiB |
| 512 KiB of nested `(` in Python / `{` in Rust (refused by the depth bound) | 126 MiB / 143 MiB |
| The same nesting at 2 MiB, above the default | 476 MiB / 561 MiB |

With both slots busy, budget about 290 MiB for the default bounds; raising
`max_file_bytes` raises this roughly in proportion. Paths must resolve to regular
files under the configured root, assuming a host-controlled workspace as for
`read`: a path that lexically leaves the root is refused before any filesystem
lookup, and symlinks are checked after canonicalization; a path beneath a
symlink that leads outside the root is refused whether or not it exists there
(see [filesystem tools](filesystem.md) for a dangling link's own target). No
filesystem writes are performed.

[Documentation index](README.md) · [Project overview](../README.md)
