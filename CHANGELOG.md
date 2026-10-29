# Changelog

## Unreleased

- Release the Python preview `0.1.0a7` with an automatic interpreter-exit barrier:
  cancel outstanding calls, reject new work and drain native bridge tasks and
  completion callbacks while releasing the GIL. Cover cancelled and abandoned
  reads, closed event loops and import-only shutdown in subprocess regressions.
  Public tool APIs and explicit process-session cleanup remain unchanged.
- Make the coding chat scenario a single `/start` command, add a host-only
  `/check` command and hide duplicate raw tool output unless `--verbose` is set.
  Explain line-based input to avoid accidentally submitting a pasted scenario
  as several independent turns.
- Add a reproducible RoomKit coding example that repairs a temporary project
  through grep/read/edit/diff/shell, observing a failing test before the fix and
  passing tests afterward. Verify real calls, unchanged tests and snapshot, the
  minimal source correction and an independent host test run. Include scripted,
  optional live-model and interactive chat modes; no tool API change is required.
- Exercise the coding scenario in installed wheel/source checks and reject
  fabricated success, missing test execution and changes to the test fixture.

- Extend the Python preview to `0.1.0a6` with `ProcessSession` and individually
  selectable `ProcessStart`, `ProcessStatus`, `ProcessRead`, `ProcessWrite` and
  `ProcessStop` RoomKit tools. Reuse the Rust manager, schemas, host limits,
  byte cursors, explicit stdin/EOF, partial writes and process-group cleanup.
- Share one lazy native registry per session across tools and event loops;
  isolate separately constructed sessions. Add explicit async lifecycle handling,
  cancellation-resistant shutdown and recovery semantics for interrupted calls.
- Validate multi-turn RoomKit use, isolation, retention, partial effects and
  cleanup. Add a runnable standalone process example and enable faulthandler
  in Python test subprocesses for native crash diagnostics.

- Extend the Python preview to `0.1.0a5` with a directly selectable `Shell`
  RoomKit tool and standalone asynchronous `ShellTool` API. The bridge calls
  roomkit-tools-process with its derived schemas, host limits, separate bounded
  streams, exit/timeout metadata and owned Unix process-group cleanup.
- Validate native/Python/RoomKit cancellation, child cleanup, loop shutdown,
  concurrent calls, environment/cwd behavior and agent execution. Add a runnable
  shell example.

- Extend the Python preview to `0.1.0a4` with independently selectable `Diff`
  and `Patch` RoomKit tools and standalone `DiffTool`/`PatchTool` APIs.
  Preserve the Rust-derived schemas, exact comparisons, structured operations,
  preview-by-default behavior, host limits, cancellation and partial results.
  Patch shares the explicit Workspace mutation scope with write/edit.
- Validate comparison and batch mutations through isolated wheel/source
  installations and RoomKit's agent loop. Add a runnable standalone patch example.

- Extend the Python preview to `0.1.0a3` with `Write` and `Edit` for RoomKit,
  standalone `WriteTool`/`EditTool` APIs, and an explicit shared `Workspace`.
  Preserve overwrite consent, exact edits, revisions, bounds, staged commits
  and the Rust cancellation boundary. Concurrent lazy initialization shares
  one Rust mutation scope across tools and Python event loops.
- Validate create/edit/remove workflows, stale revisions, concurrent mutations
  and cancelled writes through installed wheels/source builds and RoomKit.
  Add a standalone Python mutation example.

- Extend the Python preview to `0.1.0a2` with independently selectable `List`,
  `Glob` and `Grep` RoomKit tools and standalone `ListTool`, `GlobTool` and
  `GrepTool` APIs. Preserve the Rust schemas, pagination, context, host limits,
  errors and cancellation; no filesystem behavior is reimplemented in Python.
- Validate discovery in isolated wheel/source installations and a RoomKit agent
  turn using all four tools. Add a standalone Python discovery example.

- Add `from roomkit_tools import Read` for direct RoomKit registration with
  `tools=[Read(root="./workspace")]`, including lazy asynchronous initialization.
- Prepare the Python `0.1.0a1` read-only preview with a Linux x86_64 stable-ABI
  wheel, source distribution and isolated installation checks for both artifacts.
- Add an on-demand distribution workflow; it builds and validates artifacts
  without publishing packages or releases automatically.

- Scan reads in bounded chunks, retaining only the requested page, with UTF-8,
  BOM/CRLF, byte-limit and cancellation behavior preserved. Host file limits
  remain explicit; the full file is scanned to validate text and hash its bytes.
- Return content revisions from read/write/edit and accept optional
  `expected_revision` on write/edit to refuse stale mutations before writing.
- Add bounded context lines to grep results while preserving match-based
  pagination and whole-result output limits.

- Add an optional PyO3/Maturin package exposing the Rust `read` implementation
  through asyncio, with derived schemas, structured results, typed errors and
  cancellation propagation. Validate isolated wheel installation on CPython.
- Add a RoomKit Python read adapter using its public Tool protocol, preserving
  schemas, results, refusal/failure distinctions and cancellation.
- Add a runnable Python RoomKit example with a real Rust read, a scripted model
  by default and optional vLLM configuration through environment variables.

- Add runtime-independent tool definitions, derived JSON schemas, structured
  results and typed errors.
- Add independent `read`, `list`, `glob` and `grep` tools with root checks,
  pagination, cancellation and explicit input, traversal and output limits.
- Add `write` and `edit` with explicit overwrite, exact replacements, ambiguity
  refusal, byte limits, staged commits and a shared mutation workspace.
- Add `diff` and `patch`: bounded comparisons, structured add/update/delete/move
  batches, preview by default, exact source expectations and partial-effect results.
- Add foreground `shell` execution with bounded stdout/stderr, exit and signal
  status, timeouts, cancellation and owned Unix process-group cleanup.
- Add persistent `process_start`, `process_status`, `process_read`, `process_write`
  and `process_stop`: trusted owner scopes, bounded journals, byte cursors,
  explicit retention gaps, stdin/EOF, lifetimes and process cleanup.
- Distinguish read-only process observation from input and EOF in tool schemas
  and descriptions; verify exact newline bytes and normal exit metadata.
- Add individually selectable RoomKit Rust adapters with lifecycle handling,
  conversation/agent process isolation and agent-loop behavior tests.
- Add six standalone examples that check their results without a model or
  RoomKit, plus behavior tests, CI, MSRV and dependency checks.
