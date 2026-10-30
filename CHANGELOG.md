# Changelog

## Unreleased

- Align the RoomKit adapter with the engine fix for Responses stream item assembly.

- Align the optional engine integration with the Responses-compatible revision
  supporting renewable provider credentials.


- Expose host-only process observation with trusted owner, command, frozen elapsed
  time and lifecycle notifications. Output chunks do not wake hidden UI monitors.

## 0.1.0a13

- Release the five selectable agent tools and optional RoomKit Python backend.
  Include a small delegated-read example alongside the lifecycle validation demo.

- Add a parallel-agent validation scenario with isolated steering, independent
  cancellation, retained follow-up history and principal report delivery. Expose
  per-instance demo pauses and bounded turn diagnostics through public RoomKit
  hooks, without automatically replaying requests or tools. Exercise static and
  dynamic configurations, empty/limited responses and provider errors in tests.

- Make unknown-agent refusals distinguish an incorrect ID from an expired
  instance. Return bounded, caller-scoped identifier hints without automatic
  substitution or side effects. Clarify exact ID copying in agent tool schemas
  and the example, and distinguish proposed tests from executed results.

- Create missing parent directories automatically in `write`, after validating
  inputs/revisions and workspace containment. Report `created_directories`;
  other mutation tools retain their existing directory requirements.
- Add selectable `agent_followup` and `agent_send` contracts in Rust and Python.
  The RoomKit backend retains agent sessions with bounded idle expiry, starts
  new missions in their existing conversation, and queues clarifications through
  steering with explicit queuing/history receipts. Preserve scope, cancellation,
  resource ownership and result relay through the principal. Extend the example
  with follow-up/message commands and per-mission evidence checks.

- Add independently selectable `delegate_task`, `task_status` and `cancel_task`
  facades with Rust-derived contracts and host-injected Rust/Python backends.
  Keep runtime and model-provider dependencies out of the new agents crate.
- Add an optional scoped RoomKit Python backend with static agent blueprints or
  dynamic host resolution, fresh instances per delegation, bounded task retention,
  cancellation/cleanup and observed result relay to the principal. Include a
  scripted/live interactive example. Available in the `0.1.0a13` preview.

- Make background-demo read verification retain earlier valid evidence and
  report observed ranges, arguments and revisions. Give the worker immediate
  feedback when its first read omits the required method range or precedes
  the outline, without rewriting calls or changing the native Read tool.

- Return background results to the main assistant with RoomKit's `relay`
  mode, so it presents the findings in the conversation. Verify the internal
  delivery and its linked assistant reply, and distinguish worker completion
  from a pending reply. Keep terminal task commands independent of the model.

- Make the background example's terminal commands execute through RoomKit's
  host API, independently of model tool selection. Add `/list`, explicit pause
  states, fresh task launches after completion and idempotent cancellation
  feedback. Keep model task tools available for natural-language requests.

- Add a RoomKit background-analysis example with read-only Rust tools, concurrent
  conversation, task status, result delivery and cancellation. Provide scripted,
  live and interactive modes with an explicit demo pause; verify child-room
  isolation, unchanged files, launch limits and shutdown cleanup.

## Earlier previews

- Release Python preview `0.1.0a12` with independently selectable `code_outline`
  in Rust, Python and RoomKit. Analyze Rust/Python definitions, signatures and
  inclusive source ranges with Tree-sitter in the separate code crate.
- Preserve parent indexes, raw-file revisions, syntax-error reporting and
  explicit pagination/signature clipping under host-owned I/O, parser and output
  bounds. Add shared language contracts and a targeted method-repair example.

- Fix the search example's source verification: preserve query parameters,
  handle normalized URLs/fragments and give the agent immediate feedback when
  it changes a returned URL. Report incomplete sequences without a traceback.

- Pin the optional Rust adapter's engine dependencies to an immutable Git
  revision so applications can consume it without sibling checkouts.

- Release Python preview `0.1.0a11` with optional Rust `OpenAISearch` and its
  Python backend factory, usable through the existing WebSearch API. Keep the
  search model/key independent of the RoomKit agent provider.
- Issue one bounded Responses request with required web search and explicit
  token/tool-call ceilings. Preserve observed search activity, token usage,
  partial responses and explicit non-execution; no automatic continuation.
- Normalize Unicode citation spans to UTF-8 offsets, preserve every cited
  source under result limits and render visible Markdown citation links.
- Extend shared Rust/Python HTTP fixtures and RoomKit examples with cited
  answers, errors, cancellation and source-budget checks. Actual OpenAI API
  access remains an opt-in live validation with a host-provided key and model.

- Release Python preview `0.1.0a10` with modular `web_search`: an open Rust
  `SearchBackend` trait, optional Exa MCP and Ollama REST implementations, and
  directly selectable `WebSearch(backend=...)` RoomKit adapters.
- Preserve direct results, cited answers and non-execution as distinct payloads,
  with capability declarations, observed usage, provider notices and bounded
  excerpts. Only Exa/Ollama direct search is implemented in this preview.
- Extract shared HTTP destination policy and body bounds without changing
  `web_fetch` contracts. Search refuses redirects, sanitizes service failures
  and propagates cancellation through the existing Python runtime bridge.
- Compare Rust/Python results with shared HTTP/JSON/SSE fixtures; verify a
  RoomKit search-to-fetch flow and external Rust backend citation contracts.

- Release Python preview `0.1.0a9` with independently selectable `web_fetch`:
  Rust `WebFetchTool`, standalone Python `WebFetchTool`, RoomKit Python
  `WebFetch` and Rust `WebFetchToolset`. No shell, model or RoomKit dependency
  is required by the new web crate.
- Fetch UTF-8 HTTP(S) text as Markdown, text or source, preserving code and
  resolving links against the actual final URL. Expose HTTP status, byte counts
  and preview truncation; support gzip with a decoded-body limit.
- Keep exact-origin access rules and private-network exceptions on the host;
  validate and pin DNS addresses on each redirect, disable ambient proxies and
  URL credentials, and apply an overall deadline and cancellation.
- Validate real local HTTP responses across Rust and Python with shared fixtures,
  including redirects, compression, encoding, size limits and cancellations.
  Add standalone Rust and RoomKit Python examples with independent verification.

- Release Python preview `0.1.0a8` with optional Rust `OutputStore` disk copies
  for shell and persistent processes, selected by the host. Existing previews
  and result shapes stay unchanged when disabled.
- Add independent `output_read` and `output_forget` tools in Rust, Python and
  RoomKit. Byte pages preserve raw output with explicit prefix/completeness
  metadata, owner isolation, quotas and bounded writer queues. Logs survive
  process cleanup until explicit release or store shutdown; storage is ephemeral.
- Add a RoomKit example that recovers a middle diagnostic without rerunning its
  command, plus regression coverage for binary/Unicode pages, quotas, disk errors,
  cancellation, live reads and cleanup. Shared Rust/Python fixtures now cover
  16 tools through seven scenarios and 49 operations.
- Compare all 14 tools across Rust and installed Python packages using common
  fixtures, complete result envelopes, typed failure categories and final file
  bytes. Cover pagination, Unicode, exact mutations/revisions, patch preview,
  bounded shell output and persistent-process byte cursors with explicit EOF.
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
