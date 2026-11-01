# Changelog

User-visible changes by Python package version. Rust crates are versioned
separately. Entries under **Unreleased** describe the source tree and are not
part of an existing published preview.

## Unreleased

### Breaking changes

- Commands from `shell` and the process tools no longer inherit the host's whole
  environment. They receive only the variables named in `inherit_env` (by
  default `DEFAULT_INHERITED_ENV`: `PATH`, `HOME`, `USER`, `LOGNAME`, `SHELL`,
  `LANG`, `LC_ALL`, `LC_CTYPE`, `TZ`, `TMPDIR`) plus the explicit values in
  `env`. Python takes both as keywords on `ShellTool.create`, `Shell` and
  `ProcessSession`; Rust takes a `CommandEnv` through `with_env`.
- Each command now runs in its own session, detached from the host's terminal:
  a command that prompts on `/dev/tty` fails at once instead of printing into
  the host's terminal and stopping until its timeout.
- `read`, `list`, `glob`, `grep`, `code_outline`, `view_image` and `ocr` name
  files relative to the workspace (`.` for the root) instead of returning the
  host's absolute path.
- The RoomKit Python process, shell-archive and output tools scope jobs and
  archives to the calling room and agent, like the Rust adapter, and refuse
  calls made outside a RoomKit tool call. Standalone Python sessions and stores
  are unchanged.
- `code_outline` refuses sources above 512 KiB by default (previously 2 MiB):
  parsing allocates far more than the source size before limits apply.
- Jobs, output archives and plans are listed in creation order (`p2` before
  `p10`).

### Added

- Add independently selectable `docs_list`, `docs_search` and `docs_read` in Rust,
  Python and both RoomKit adapters, over injected documentation backends.
- Include an optional file backend with bounded immutable Markdown/text snapshots,
  literal ranked search, source lines, provenance, pagination and revision checks.
  Add scripted/live RoomKit examples; HTTPS is not implemented in this version.
- `grep` takes a host `max_line_bytes` (64 KiB by default). Longer lines are
  skipped and counted in `skipped_lines`, and a match too large for a page is
  returned alone with `clipped`. `list`, `glob` and `grep` skip names that are
  not UTF-8 and count them in `skipped_entries`.
- `ForkedProcessError` is raised by native calls in a child process forked
  after the parent used the package.
- Python hosts reach the jobs and archives of any owner, as Rust hosts do:
  `ProcessSession` and `OutputStore` methods take `owner=` (`None` keeps the
  standalone owner), `ProcessSession.close_owner(owner)` stops and releases one
  owner's jobs, and `roomkit_owner(room_id, channel_id)` returns the key the
  RoomKit tools use, in Python and in the Rust adapter.
- `roomkit_tools_core::WorkspaceRoot` holds the path rules shared by the
  root-configured tools; `resolve(path, operation, canonicalize)` applies them
  with the filesystem access the caller supplies.
- `roomkit_tools_core::identifiers::creation_order` orders sequential
  identifiers as they were created.

### Fixed

- A deeply nested brace glob no longer panics `glob` and `grep`.
- `grep` and `glob` match on a cancellable worker, so a costly pattern no
  longer blocks the async runtime and cancellation stops it between entries.
- Paths outside the workspace are refused before any filesystem call, including
  paths that leave the root and come back (`../x/../root/a`) and paths beneath
  a workspace symlink that points outside, so the answer no longer reveals
  whether an outside file or directory exists (only a dangling workspace
  link's own, host-chosen target remains distinguishable). A root given with `..` after a
  symlink no longer accepts absolute paths in an unrelated directory.
- `write` and `patch` create new files with the umask-derived mode instead of
  0600.
- For files whose lines all end in CRLF, `edit` and `patch` accept the LF text
  that `read` shows. A line ending in a bare CR before the CRLF still needs its
  exact ending.
- `web_fetch` bounds node count, depth and merged attributes while parsing
  HTML, and refuses a page whose Markdown would exceed the limit before
  converting it, measuring tables and math spans as the converter writes them.
  Small adversarial pages no longer allocate gigabytes, and pages of nested
  layout tables are not refused below the limit.
- The Exa backend splits results only at well-formed record headers: a
  Markdown rule in a page no longer fails the search or starts a new hit. A
  record whose URL is not a usable source is dropped with a notice and
  `partial`.
- The RustO OCR engine decodes with the dictionary bytes that were size-checked
  and fingerprinted, instead of reading the file a second time.
- OCR errors no longer name `view_image`.
- The output store directory is created with mode 0700.
- A finished command keeps its result when the final process-group signal is
  refused. A command whose shell cannot be signalled (for example one running
  under other credentials) no longer holds a timed-out or cancelled call: the
  result reports no exit status and incomplete output, and a persistent job
  ends as failed.
- A child forked after the package was used no longer hangs on every native
  call and at exit: calls raise `ForkedProcessError` and the exit hook returns
  at once. A call that completes after its event loop closed no longer prints a
  traceback. Forked children are detected without an at-fork hook, so forking
  cannot block inside `os.fork()`, and `close()` in a forked child is refused
  before it changes any state. A fork that happens while native calls run can
  still, rarely, block the child on a lock held by a native thread; prefer the
  `spawn` start method. The optional OCR engine package behaves
  the same way and raises the base package's `ForkedProcessError`; it now
  requires the matching base version.
- `Shell` keeps its environment options as validated at construction, and the
  RoomKit tools refuse room or channel ids that are not Unicode text.
- `ViewImage`, `CodeOutline` and `Ocr` take their limits as explicit keywords,
  so a misspelled option fails at construction.
- Documentation tools let I/O errors propagate in RoomKit like the other tools.
- Tool exceptions report `roomkit_tools` as their module and can be pickled;
  native contract classes report `roomkit_tools._native`.

### Packaging and documentation

- The wheel ships the license notices of every native dependency
  (`licenses/native_dependencies.txt`), checked against the lock file.
- Distribution checks admit only tracked files in source archives and expected
  paths in wheels.
- The guides state that stopping a process sends SIGKILL to its group, what
  replacing a file by rename does not keep, and the measured peak memory of
  `code_outline` and `view_image`.
- Align the optional Rust adapter with the current RoomKit engine revision,
  retaining individually selected tools and host-owned lifecycles.


## 0.1.0a15

- Add independently selectable plan CRUD in Rust, Python and both RoomKit adapters,
  with injected storage, a bounded memory backend, trusted conversation/agent
  scopes and atomic revision checks. Plans record progress without executing work.

- Add selectable `ocr` contracts, bounded native execution and positioned text
  results in Rust/Python, with RoomKit adapters for both languages.
- Supply RustO/RTen as a separate Rust crate and optional Python distribution,
  with explicit local models, private thread pools and shared engine lifetimes.
  Neither model downloads nor inference dependencies are imposed on the base
  Python package. Support EXIF orientation and explicit clockwise rotation.
- Raise the workspace and Python source-build minimum to Rust 1.94.

## 0.1.0a14

- Add `view_image` in Rust and Python for static PNG, JPEG and WebP, with bounded
  PNG previews, orientation handling, source revisions and unchanged source files.
  The RoomKit Python adapter returns typed image content; the pinned RoomKit Rust
  adapter does not yet support image tool results.
- Add host-only Rust process observation with trusted owners, commands, elapsed
  runtime and lifecycle notifications. Output chunks do not trigger notifications.
- Reorganize the public README, guides, examples and contribution instructions.
  Add package documentation/license metadata and a first-PyPI-release plan.
  Check description rendering in the distribution workflow.

## 0.1.0a13

- Add independently selectable `delegate_task`, `task_status`, `cancel_task`,
  `agent_followup` and `agent_send` with Rust-derived contracts and injected backends.
- Add the RoomKit Python agent backend with static profiles or dynamic host
  resolution, isolated instances, bounded retention, cancellation and cleanup.
- Retain idle agents for follow-up missions and queue messages to active workers.
  Reports return through the principal; worker completion and delivery are
  reported separately. Unknown-agent errors include bounded, caller-scoped hints.
- Add delegated-read, background and parallel-agent examples, with observed tool
  evidence, isolated steering, cancellation and retained follow-up history.
- Create missing parent directories in `write` after input/revision/root checks;
  report `created_directories`. Other mutation tools retain existing-parent rules.

## 0.1.0a12

- Add `code_outline` in Rust, Python and RoomKit for Rust/Python source definitions,
  signatures, nesting and inclusive line ranges using Tree-sitter.
- Preserve revisions across pages and report recovered syntax errors, shortened
  signatures and parser/resource bounds. Add a targeted method-repair example.
- Make search-example verification preserve returned URL query parameters and
  report incomplete search/fetch sequences without treating them as success.
- Pin optional Rust RoomKit dependencies to an immutable engine revision.

## 0.1.0a11

- Add the OpenAI Responses search backend behind the existing `WebSearch` API.
  Keep its model and credentials separate from the main agent provider.
- Preserve observed search activity, cited answers, partial responses, usage and
  explicit non-execution. Normalize citation spans to UTF-8 byte offsets and
  render clickable source links. Apply token/tool-call ceilings without retries.

## 0.1.0a10

- Add modular `web_search` with an open Rust `SearchBackend` trait, optional Exa
  MCP and hosted Ollama REST backends, and selectable RoomKit adapters.
- Define direct results, cited answers and non-execution as distinct payloads.
  Bound queries, excerpts, responses and deadlines; keep credentials host-owned.
- Extract shared DNS/origin/body controls without changing `web_fetch` contracts.

## 0.1.0a9

- Add `web_fetch` in Rust, Python and RoomKit for UTF-8 HTTP(S) text, Markdown and
  HTML source. Preserve code, resolve links, follow validated redirects and
  report status, ingestion sizes and output truncation.
- Enforce host origin policy, DNS-pinned connections, private-network exceptions,
  decompressed-body limits and cancellation. Disable ambient proxies and URL credentials.

## 0.1.0a8

- Add optional `OutputStore` disk copies for shell and persistent processes,
  plus independently selectable `output_read` and `output_forget` tools.
- Provide byte cursors, completeness metadata, owner isolation, quotas and
  bounded writer queues. Logs survive process-record removal and remain temporary.
- Add shared Rust/Python contract scenarios and exact archived-output verification.

## 0.1.0a7

- Stabilize native Python shutdown by cancelling outstanding calls and draining
  Python-facing tasks and completion callbacks before interpreter finalization.
- Add a reproducible coding example with real failing/passing tests, revision-aware
  edits, diffs and independent verification. Interactive commands submit one task
  at a time and distinguish model claims from observed effects.

## 0.1.0a6

- Expose `ProcessSession` and the five selectable process tools in Python/RoomKit.
  Share one native manager per session with explicit lifetime and owner isolation.
- Preserve byte cursors, exact stdin/EOF, partial writes and process-group cleanup.
  Add cancellation-resistant shutdown and recovery guidance for interrupted calls.

## 0.1.0a5

- Expose `Shell` and the asynchronous `ShellTool` API with bounded separate output,
  exit/signal/timeout metadata, cancellation and Unix process-group cleanup.

## 0.1.0a4

- Expose `Diff`/`Patch` and their standalone Python APIs. Preserve structured
  operations, preview by default, exact source expectations and partial-effect results.
  Share the mutation workspace with write/edit.

## 0.1.0a3

- Expose `Write`/`Edit` and their standalone Python APIs with a shared `Workspace`,
  explicit overwrite, exact replacements, content revisions and staged commits.
- Preserve cancellation boundaries and stale-revision refusals across Rust/Python.

## 0.1.0a2

- Expose `List`, `Glob` and `Grep` in Python/RoomKit, preserving pagination,
  matching context, host bounds and Rust validation.

## 0.1.0a1

- Introduce the Python read preview with PyO3/Maturin, an async `ReadTool` API and
  a directly selectable RoomKit `Read` adapter.
- Distribute a Linux x86_64 stable-ABI wheel and source archive with independent
  installation checks and an on-demand build workflow.
- Establish the standalone Rust file/process tools, derived contracts, typed
  errors, content revisions, bounded reads, grep context and optional RoomKit adapters.
