# Changelog

User-visible changes by Python package version. Rust crates are versioned
separately. Entries under **Unreleased** describe the source tree and are not
part of an existing published preview.

## Unreleased

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
