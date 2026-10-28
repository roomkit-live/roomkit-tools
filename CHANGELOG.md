# Changelog

## Unreleased

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
