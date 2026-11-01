# Filesystem tools

All paths are resolved beneath a host-selected workspace. A path that lexically
leaves it (an absolute path elsewhere, or `..` past the root) is refused before
the filesystem is consulted, so the error is the same whether or not the target
exists. Accepted paths are then canonicalized, and a symlink resolving outside
the root is refused. Every path in a result is workspace-relative, with `/`
separators and `.` for the root; the host's absolute paths are not returned.
These checks assume a host-controlled filesystem; they do not provide an OS
sandbox against concurrent path replacement. Select RoomKit classes such as `Read` and `Edit`, or use their
standalone `ReadTool` and `EditTool` counterparts. Examples below assume an async
function and an existing `./workspace` directory.

## Shared mutation scope

`Workspace` is an explicit shared root and mutation scope. Passing the same
object to `Write`, `Edit` and `Patch` serializes their read/prepare/commit operations,
including concurrent first calls and reuse across event loops. Read-only tools
accept it as a path and do not acquire that lock. A plain path also works, but
each mutation tool then owns an independent scope; equal path strings do not
share a global lock. External editors and separately created workspaces are not
coordinated by this object.

## Reading text

`ReadTool` returns complete lines, one-based numbers and a continuation
offset. Defaults are 2,000 lines, 50 KiB of rendered output and 8 MiB of
file input. It rejects non-text files, files above the ingestion bound,
and an individual line that cannot fit in a page. It scans in 64 KiB chunks,
retaining only the page and a bounded current line. Raise
`ReadLimits::max_file_bytes` explicitly for larger files; the default remains
8 MiB. The entire file is still scanned to validate UTF-8/NUL and compute a
`revision`, including when only a small page was requested. Cancellation can
interrupt the scan between chunks. The host owns the root and limits.

```python
from roomkit_tools import ReadTool

read = await ReadTool.create("./workspace")
page = await read.read("notes.txt", offset=1, limit=20)
print(page["text"])
print(page["data"]["revision"], page["data"]["next_offset"])
```

Reconstruct source from `data.lines` when needed; `text` adds line numbers for
presentation. A page may not contain the whole file. The `revision` hashes all
raw source bytes, including BOM and line endings.

## Directory listing, globbing and text search

The standalone discovery tools use the same construction and result shape:

```python
from roomkit_tools import GlobTool, GrepTool, ListTool

# Inside an async function:
listing = await ListTool.create("./workspace", max_entries=100)
glob = await GlobTool.create("./workspace")
grep = await GrepTool.create("./workspace", max_context_lines=5)

directories = await listing.list()
paths = await glob.glob("**/*.txt")
matches = await grep.grep("blue", include="**/*.txt", context_before=2)
```

`list`, `glob` and `grep` accept a one-based `offset` and optional `limit`;
resume with `data.next_offset`. Grep offsets count matching lines, not context.
The searched `path`, discovered entries and matches are workspace-relative and
usable by `read`. Every page rescans; results are not snapshots across external edits.
Hidden entries are included, ignore files are not applied, and child symlinks
are not traversed. Grep supports Rust regex syntax, `literal=True`,
`ignore_case=True`, `include`, `context_before` and `context_after`.
It skips binary/non-UTF-8 files and reports `data.skipped_files`. All three
tools skip entries whose names are not UTF-8, without searching a skipped
directory, and report them in `data.skipped_entries`. A glob pattern or grep
`include` without `**` or a `[...]` class only scans as many directory levels as
it has components: `*.rs` reads only the search directory, so unrelated large or
deep subtrees do not exhaust the scan bounds.

Grep reads and matches files on a blocking worker, never on the async
executor. Lines longer than `max_line_bytes` are not handed to the regex: they
are not searched, can still appear as context, and are counted in
`data.skipped_lines`. Cancellation, or dropping the call, returns at once; the
worker checks cancellation before each file and each line, so it stops after
at most one line of up to `max_line_bytes`. A match that does not fit
`max_output_bytes` together with its context ends the page before it; when it
is the first match of a page it is returned alone with `clipped: true`, its
context omitted and its `text` cut at a UTF-8 boundary to fit, so the next
offset always moves past it.

Host bounds are explicit constructor keywords for both APIs:

| Tool | Host bounds and defaults |
| --- | --- |
| `List` / `ListTool` | `max_entries=200`, `max_output_bytes=51200`, `max_scan_entries=10000` |
| `Glob` / `GlobTool` | `max_matches=200`, `max_output_bytes=51200`, `max_scan_entries=10000`, `max_depth=32` |
| `Grep` / `GrepTool` | Same glob bounds, plus `max_file_bytes=8388608`, `max_total_bytes=33554432`, `max_context_lines=20` per side, `max_line_bytes=65536` |

Omitted bounds retain Rust defaults. Bounds must be positive, except
`max_context_lines=0` disables context. Traversal or ingestion limits fail the
call; output pagination is reported explicitly. Run the standalone
[discovery example](../examples/discover_files.py) with
`.venv/bin/python examples/discover_files.py`; it needs no RoomKit or model.

## Creating and editing files

For standalone mutations, share one workspace and pass the latest revision:

```python
from roomkit_tools import EditTool, ReadTool, Workspace, WriteTool

# Inside an async function:
workspace = Workspace("./workspace")
read = await ReadTool.create(workspace)
write = await WriteTool.create(workspace)
edit = await EditTool.create(workspace)

created = await write.write("notes.txt", "Status: draft\n")
before = await read.read("notes.txt")
updated = await edit.edit(
    "notes.txt", "draft", "ready", expected_revision=before["data"]["revision"]
)
```

`write(path, content, overwrite=False, expected_revision=None)` writes complete
UTF-8 content. Replacing an existing file requires `overwrite=True`, including
when a matching revision is supplied. `edit(path, old_text, new_text,
replace_all=False, expected_revision=None)` replaces an exact nonempty passage.
Multiple matches require `replace_all=True`; no matches are refused. Empty
`new_text` removes the passage and keeps the file. Both preserve raw line endings
and BOM, and refuse leaf symlinks and NUL/binary content. `write` automatically
creates missing parent directories (up to 64 levels) inside its root and returns
them in `created_directories`. Input/revision checks run before directory creation;
parents can remain if subsequent I/O or cancellation prevents the file commit.
`edit` requires an existing file; patch operations still require existing parents.

`read` shows each line without its line ending, so a CRLF file reads like an LF
one. When every line ending of the file being edited is CRLF, `edit` takes LF
endings in `old_text` and `new_text` as CRLF, and patch does the same for
`old_content` and `new_content`; text already written with CRLF is unchanged,
and the file stays CRLF throughout. A file with mixed or LF endings is matched
exactly.

Both constructors accept `max_file_bytes` (8 MiB by default), bounding source,
arguments and result as applicable. Results contain changed/created flags or
replacement counts, byte counts and the resulting content revision. The optional
`expected_revision` checks the destination inside the shared mutation lock before
writing; a stale revision is refused even when the selected text still matches.
It requires an existing destination, so omit it for creation. This is not an OS
compare-and-swap against external editors. Read before editing or overwriting,
then use the revision returned by read/write/edit.

Every mutation stages the complete new content in a temporary file beside the
destination, then renames it into place. A file created by `write` or a patch
`add` gets the mode of ordinary file creation (on Unix, 0o666 minus the process
umask, as with `std::fs::write` or Python's `open`). A replacement keeps the
destination's permission bits, and a move keeps its source's.

Cancellation requests stopping before commit. A commit that has already started
may finish even if Python receives `CancelledError` or RoomKit times out; the
Rust worker keeps the shared lock until completion. Read the file before retrying
an interrupted mutation. See the standalone [mutation example](../examples/edit_file.py),
run with `.venv/bin/python examples/edit_file.py`, for a complete flow including
stale-revision refusal.

## Comparing and applying changes

Compare two existing files and preview or apply a batch:

```python
from roomkit_tools import DiffTool, PatchTool, Workspace

workspace = Workspace("./workspace")
diff = await DiffTool.create(workspace)
patch = await PatchTool.create(workspace)
comparison = await diff.diff("current.txt", "proposed.txt", context_lines=3)
print(comparison["text"])

changes = [
    {"op": "update", "path": "current.txt",
     "old_content": "count=2\n", "new_content": "count=3\n"},
    {"op": "add", "path": "summary.txt", "content": "Count increased.\n"},
]
preview = await patch.patch(changes)  # dry_run=True: no files are changed.
print(preview["text"])
result = await patch.patch(changes, dry_run=False)
print(result["data"]["completed"], result["data"]["applied"], result["data"]["failure"])
```

`DiffTool` returns complete structured hunks and unified-style text. It compares
exact UTF-8 bytes, including BOM and line endings, without consulting Git or
modifying files. Its two reads are not a transactional snapshot. Defaults:
8 MiB/file, 20,000 lines/file, 1,000,000 comparison cells, 50 KiB rendered output
and 100 context lines/side. Constructor keywords `max_file_bytes`, `max_lines`,
`max_comparison_cells`, `max_output_bytes` and `max_context_lines` set those
bounds; context may be zero. Oversized comparisons fail instead of clipping.

`PatchTool` accepts structured `changes`, not textual Git/Codex patches or the
text returned by `diff`. Operations are `add` (`path`, `content`), `update`
(`path`, `old_content`, `new_content`), `delete` (`path`, `old_content`) and
`move` (`from`, `to`, `old_content`, optional `new_content`). Omitted/null move
content preserves the source bytes. Existing sources must match the complete
`old_content` exactly, apart from the CRLF rule above; this is patch's
stale-content check, rather than an
`expected_revision` argument. A numbered or truncated `read` display cannot be
used verbatim as that content. Paths must be distinct, parents must exist and
add/move destinations must be absent.

All changes are prepared before writing. An omitted `dry_run` means preview;
explicit `False` applies a freshly validated batch. Previews do not reserve files.
Defaults are 32 operations, 16 MiB combined argument bytes, plus the diff bounds
above; `max_output_bytes` caps the combined prepared previews. Constructor
keywords also include `max_changes` and `max_input_bytes`. Preview context is
three lines capped by `max_context_lines`.

Inspect `completed`, `applied` and `failure`: batches do not promise multi-file
rollback. A move records destination creation and source removal separately.
Cancellation may leave committed effects; reread affected files before retrying.
The shared Workspace serializes patch with write/edit, including preparation
and preview. See [the runnable comparison/patch example](../examples/patch_files.py):
`.venv/bin/python examples/patch_files.py`.

[Documentation index](README.md) · [Project overview](../README.md)
