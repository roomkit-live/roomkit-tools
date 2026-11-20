# Changelog

User-visible changes by Python package version. Rust crates are versioned
separately. Entries under **Unreleased** describe the source tree and are not
part of an existing published preview.

## Unreleased

- The schema descriptions of shell's `timeout_ms` and `run_in_background`
  and of process_read's `wait_ms` no longer break a sentence with a line
  break, which came from doc comments wrapped over two lines.

- `process_start`, a shell command started with `run_in_background`, and one
  moved to the background at its timeout say how long the process may run,
  counted from its start: `p1 started; stopped after 300000 ms unless it ends
  first; use process_read for output`. Nothing told a model that its server
  would be stopped after five minutes.

- With `background_on_timeout`, a shell `timeout_ms` above the host maximum
  is capped to it rather than refused, and the result's text says so
  (`timeout_ms 300000 capped to the host maximum of 120000 ms`): the
  command runs until the maximum, then moves to the background. Without it,
  the refusal of a shell that has a process session now adds `; for a longer
  job, set run_in_background`.

- Two refusals of the shell and process tools give the bound and the way
  out. A command above `max_command_bytes` reads `command is 20034 bytes,
  over the host limit of 16384: write the script to a file, then run it`,
  for shell and process_start alike. A `process_read` offset past its
  stream's end names that stream and its end: `stderr_offset 5 is past
  stderr's end 2; each stream has its own next_offset`, where it read
  "output cursor is past the observed stream end".

- A shell command moved to the background closes its shell archives at the
  move as complete: every byte until then is stored, where the archive said
  `complete=false, error="capture_incomplete"`. The result's text now says
  that `process_read` reads what the command writes after the move, each
  stream from offset 0, so a model no longer passes the shell's
  `total_bytes` as an offset and meets "output cursor is past the observed
  stream end".

- At a process quota, a new process, or a shell command moved to the
  background, first releases the record of its owner that finished first,
  as the output store does for archives: the record's archives stay, and
  the latest end, which the model was just told to read, is kept. Finished
  records filled the quota (8 per owner by default) until
  `process_stop(forget=true)`, so after eight moved builds the ninth long
  command was killed (`signal=9`) instead of moved, and `process_start` was
  refused. A start is refused only when every job of the owner still runs, or
  the session is full of running jobs and other owners', in words that say
  which. A record released at a quota or forgotten is refused to its owner
  as such, `p1 was released: a finished process makes room at the quota or
  when forgotten; its archives, if any, remain readable with output_read`,
  where it read as an unknown process; another owner still reads it as
  unknown.

- With `keep_background_jobs`, a shell command moved to the background at
  its timeout keeps its background jobs as in the foreground: once its shell
  exits on its own, they run on, and the shell's `ends()`, `close()` and
  `detach()` tell their end, stop them and let them go. They were killed
  with the moved command's shell, so `cargo build && (./server > log 2>&1 &)`
  lost its server whenever the build outlived the timeout. A moved command
  still running when the shell closes keeps no job: those it leaves once its
  shell exits are stopped, untold; after `detach()` they run on.

- `plan_create`'s `text` gives the plan's ID and revision and its steps' IDs
  with their statuses, `plan-1 (revision 1) created` then
  `1. inspect [in_progress]`, rather than echoing the title, objective and
  descriptions the model just wrote (5.5 KB for a 17-step plan). `data` is
  still the whole plan.

- The `text` of `delegate_task`, `task_status`, `cancel_task`,
  `agent_followup`, `agent_send` and `ask_user` is lines rather than the JSON
  of `data`, which is unchanged: a task's quoted IDs and states on one line
  and its result, up to 8 KiB, on its own indented lines instead of one
  escaped string, a lone `\r`, U+2028 and U+2029 breaking lines as `\n`
  does; a message's outcome; one line per answer.

- Clearer argument descriptions: `plan_create`'s `title` is the short name
  `plan_list` shows, a few words (a model took `objective` for the whole
  plan and sent `objective_title` for the title); `ask_user`'s `header`
  aims at 12 characters and gives the 64 bytes the check enforces, where it
  announced a 12-character limit; `plan_update`'s `step_changes`
  description holds no line break.

- The input schemas give the bounds the tools enforce: `minimum: 1` for
  `plan_update`'s and `plan_delete`'s `expected_revision`, which offered 0,
  a refused revision, and `minimum: 1` with `maximum` 100, 50 and 500 for
  the `limit` of `docs_list`, `docs_search` and `docs_read`.

- A `plan_update` by `step_changes` that changes a step's status alone
  lists that step as `3. s3 [completed]`, without the description the model
  just left unchanged.

- A plan step's description over 2,048 bytes is refused naming the step and
  both sizes, `step agent-par-conversation: description is 2071 bytes, at
  most 2048`, where "step description exceeds 2048 UTF-8 bytes" left the
  model to guess which of its steps to shorten.

- `docs_list` and `docs_search` lines leave a document's revision to `data`
  and show its source only when it is not the document ID: the two took
  about half of a list's `text`. `docs_read` still names both, and the
  descriptions say so: `docs_list` lists IDs, titles and versions, and
  `docs_read`'s `expected_revision` comes from a previous `docs_read`.

- A `FileDocs` document's title is its first `# ` heading outside fenced
  code: a `# fetch the sources` comment in a ```` ```sh ```` block above the
  heading became the title. A fence closes as Markdown closes it, on a run
  of its character at least as long with nothing after it, and a run
  indented four spaces is no fence.

- `code_outline`'s `text` gives one line per symbol, its index, range,
  parent and signature with whitespace collapsed and comments left out, so
  that no line comment swallows what follows it
  (`2 [2-3] ^1 def calculate_fee(self, subtotal):`), where each symbol took
  a heading line and its header as written, over several lines for a long
  one. A clipped signature, which may end before the declaration behind a
  long decorator or attribute, is followed by the kind and name:
  `[signature_truncated: function "test_conversion"]`. `data` is unchanged.

- `code_outline` on a file of another extension answers
  `code_outline reads .rs, .py and .pyi files; use read for Makefile`, where
  it told the model to set `language`, which only parses the file as Rust or
  Python; on a directory it answers that it accepts regular files only,
  whatever the directory's name. The `language` description says to omit it
  for `.rs`, `.py` and `.pyi` files.

- `code_outline`'s range of a Rust item covers its outer doc comments
  (`///`, `/** */`) and the attributes written before or among them: a
  derive written before the item's doc comment, and the doc comment itself,
  fell outside `[start_line, end_line]`. Plain comments stay outside; the
  signature is unchanged.

- `ocr`'s `max_output_bytes` bounds the returned text, each kept block's text
  counted once with the newlines joining them, rather than the whole JSON
  envelope, which repeats that text three times and adds every block's
  geometry: a dense page of 200 blocks and 20 KB of text, which filled the
  default 64 KiB and lost 42 blocks, now returns them all. Blocks past it are
  still omitted whole; `max_blocks` bounds the geometry. A host that hands the
  whole envelope to a model gets more than before: about three times the text
  as JSON escapes it, plus about 150 bytes per block, up to about 235 KB at
  the defaults for plain text and 430 KB for text full of quotes, where the
  envelope was at most 64 KiB.

- `view_image` previews an ordinary 4K photograph instead of refusing it: a
  PNG preview over `max_image_bytes` is made again with an edge three
  quarters as long until it fits, and `resized` is set; at the defaults, a
  detailed 3840×2160 photo gets, for example, 1536×864. An image whose preview fits at no size
  fails with `the image's PNG preview exceeds the host's N-byte limit at every
  size`, which no longer tells the model to change host settings.

- `web_search` refuses a query or limit naming the field and what it
  accepts, such as `limit must be between 1 and 10` (the host's
  `max_results`, or the backend's ceiling when lower) or `query must not be
  empty`, where it said `query or limit exceeds search constraints`. Its input
  schema gives `limit` a `minimum` of 1.

- `web_search`'s direct results fit the host's output budget: past it, each
  result that does not fit even without its excerpt (such as one with a huge
  title) is left out, then every excerpt is shortened to the longest common
  length that fits, each cut counted in `omitted_bytes` or `omitted_results`
  and the result marked `partial`. With roomkit-code's 48 KiB budget, a search for 6 results
  or more used to fail outright. The `text` lists the results as lines,
  `N. Title — URL`, then about 600 bytes of excerpt, rather than the JSON of
  `data`, which keeps every excerpt; an answer's `text` is unchanged.

- `web_fetch` reads JavaScript, YAML, TOML, shell scripts and
  newline-delimited JSON served under `application/` (and `+yaml` types), a
  body without a media type when it is UTF-8 without NUL (as plain text,
  never converted from HTML, and refused when it declares another charset),
  and an ASCII body declared Latin-1 or windows-1252; it refused all of them.
  A response refused for its Content-Type, charset, content encoding or
  bytes, or for a body over `max_response_bytes`, now names its Content-Type
  and HTTP status, such as `response type is not supported text
  (Content-Type: application/octet-stream, HTTP 404)`, so a 404 no longer
  loses its status.

- `web_fetch` and the search backends reach the public addresses of
  192.0.0.0/16, such as WordPress.com's 192.0.78.0/24: only its
  special-use blocks, 192.0.0.0/24 and 192.0.2.0/24, are refused as
  non-public, where the whole /16 was.

- The descriptions of `grep`'s `include` and `edit`'s `old_text` no longer
  hold a line break from a wrapped doc comment.

- Three refusals say what to do: `read` past the end gives the file's line
  count (`offset 9 is beyond the end of the file, which has 3 lines`);
  `write` with `expected_revision` on a missing file says `the file does not
  exist; omit expected_revision` instead of that the file changed; and `edit`
  whose `old_text` starts with `read`'s line number (`12: `) names it.

- `grep` caps `context_before` and `context_after` at `max_context_lines`
  (20 by default), as `diff` caps its context, and the facts line states the
  context used, `[context_before=20, context_after=20]`: asking for 30 lines
  failed the search with `LimitExceededError`.

- A `patch` preview never refuses the patch for its size: adding or deleting
  a 60 KiB file with `dry_run=False` failed with "diff exceeds
  max_output_bytes", where `write` of the same text worked. The preview that
  reaches `max_output_bytes` is cut at a line and ends
  `[clipped=true, additions=N, deletions=M]`, and past `max_comparison_cells`
  a preview shows the changed lines replaced whole and says so,
  `[compared=whole]`. A preview's `text` starts
  `preview only: nothing written; call again with dry_run: false` instead of
  `completed=true`, and an applied patch's `text` lists its effects, and
  after a failure the changes never applied, without repeating the diffs,
  which `data.previews` still holds.

- `diff` and `patch` compare lines with a Myers diff instead of a full
  longest-common-subsequence table, so two changes far apart no longer fail:
  in a 1,500-line file, changing lines 3 and 1,498 failed `diff` and an
  applied `patch` with "diff exceeds max_comparison_cells". The script is
  still a shortest one. `max_comparison_cells` keeps its name and default and
  now bounds the comparison's work: each line between the common first and
  last lines, then each step of the search, which grows with the number of
  changed lines rather than with the files' length. Reordered lines cost
  more than they did: at the default bound, 707 to 999 lines in reverse
  order, which the table compared, are now refused. An empty side costs
  nothing.

- A `glob` pattern, or a `grep` `include` with `/`, is scanned from its
  leading directories without glob syntax: `src/**/*.rs` scans `src` only,
  where a 10,001-file `data/` beside it used up `max_scan_entries`, though
  `**/*.rs` with `path="src"` worked. The matches are those a scan from
  `path` gives.

- `list`, `glob` and `grep` no longer fail at `max_scan_entries` or
  `max_depth`: they return what they found, sorted, with `truncated: true`,
  and the facts line names the bound and the path where the scan stopped,
  `[truncated_by=max_scan_entries, stopped_at="data"]`. Glob and grep walk
  directories in path order: the first ones whole, then, in the directory
  where the bound is reached, the entries read until then in the
  filesystem's order. A directory below `max_depth` is listed but not
  entered. A flat directory over the bound, which failed every unrestricted
  search, now lists the entries read. `next_offset` pages through what was
  found, so `truncated` no longer implies a `next_offset`: a host that pages
  while `truncated` must stop when `next_offset` is null. When the walk and
  `max_total_bytes` both stop `grep`, the facts name the stop first in path
  order.

- `grep` and `glob` also leave out what a Git repository's
  `.git/info/exclude` excludes (a worktree's is its repository's) and, when
  the workspace is a directory below the repository's root, what the ignore
  files of the directories between them exclude: a `target/` named there
  made the scan exceed `max_scan_entries`. An ignore file over its bounds is
  named in the error, as is one above the workspace (`../.gitignore`). A
  `.git` file is followed only to a directory holding a regular `HEAD`, only
  regular files are opened, and a store that cannot be read adds no rules.

- Once `grep` has read `max_total_bytes` (32 MiB by default), it returns the
  matches found so far with `truncated: true` and no `next_offset`, and its
  facts line names the file it stopped at,
  `[truncated_by=max_total_bytes, stopped_at="logs/32.txt"]`, instead of
  failing: every page reads from the first file, so in a tree over 32 MiB of
  text a deep page or a rare pattern failed.

- `grep` skips a file over `max_file_bytes` and counts it in
  `skipped_files`, as it does a binary file, instead of failing the whole
  search: a 9 MiB PDF beside the sources made every search fail. A file is
  known as binary from its first 8 KiB; the rest of it is neither read nor
  counted toward `max_total_bytes`. The facts line names the first file
  skipped, `first_skipped_file="scan.pdf"`.

- A shell command can run on in the background as a process. With
  `ShellTool.create(processes=session)` (Rust: `ShellTool::with_processes`),
  `shell(..., run_in_background=True)` starts it as one of the session's
  processes and returns its `process_id` at once, and
  `background_on_timeout=True` (Rust: `ShellLimits::background_on_timeout`)
  moves a command still running at its timeout there instead of stopping it,
  with no byte lost. The result's new `process_id` names it; the session's
  process tools read, stop and tell it. The tool description steers models to
  `run_in_background` rather than `&`.

- A host is told when a background job ends, without polling it:
  `ProcessSession.ends()` and `ShellTool.ends()` are async iterators (Rust
  `Stream`s) of dicts (`JobEnd`: owner, tool, process_id, command, cwd, end,
  exit_code, signal, elapsed_ms). `ShellTool` follows the jobs a command
  leaves running with `keep_background_jobs` until their process group is
  empty, and gains `close()`, which stops them, and `detach()`, which lets
  them go; dropping it leaves them running, as before.
  `notify_job_ends(kit, ends)` tells each RoomKit agent the end of a job it
  started, as an instruction that gives it a turn or is read by its running
  turn; a stop is not told. The `Shell` adapter files a call's jobs under its
  caller even without an output store.

- A tool's `input_schema` lists its variants under `anyOf` rather than
  `oneOf`, with the same variants: Kimi K2.7 refused the `$ref` to a `oneOf`
  in `code_outline`'s, `patch`'s and `web_fetch`'s schemas.

- `plan_update` also takes `step_changes`, `[{step_id, status?, description?}]`:
  only those steps change, the others and their order are kept, and the
  result's `text` gives the new revision and the changed steps alone: changing
  one status of an 18-step plan no longer sends the whole list, and no longer
  returns it to a model that reads the `text` alone, as RoomKit's adapters now
  give it. `steps` still replaces the whole list; a call gives one or the
  other. `PlanUpdateTool` applies them to the plan its backend reads, so a
  backend receives the complete `steps` as before; `MemoryPlanBackend.update`
  also takes `step_changes`. The `text` of every plan and documentation
  result is now lines a model reads (a plan's numbered steps, a document's
  numbered lines) rather than the JSON of `data`, which is unchanged.

- `process_read` takes `wait_ms`: the read first waits, up to that many
  milliseconds, until the process ends or writes past the given offsets, so an
  agent awaits a build in one call instead of polling it one tool round at a
  time. The host caps it with `max_wait_ms` (20 s by default, below RoomKit's
  30 s tool-call timeout; Python `ProcessSession(max_wait_ms=...)`); a longer
  wait is capped, and the page says whether the process still runs. Without
  `wait_ms`, a read is unchanged.

- `grep` and `glob` leave out what the workspace's `.gitignore` and `.ignore`
  files exclude, from the root down to each directory, and `.git`, without
  entering them: at the root of a built project they answer instead of
  exceeding `max_scan_entries` on `target/`. `include_ignored=True` scans
  everything, and a `path` searched explicitly is searched even when ignored.
  Hidden files are still included, and `list` is unchanged. The rules are read
  with the `ignore` crate's gitignore matcher, from ignore files of at most
  256 KiB each and 2 MiB per scan, compiled off the async executor; as in
  ripgrep, `.ignore` rules decide before `.gitignore` ones at every level, and
  an ignore file the scan cannot read fails it naming the file.

- A path inside the workspace that names nothing is invalid input naming it,
  `Invalid input: no such file or directory: src/absent.rs`, for read, list,
  grep, glob, diff, edit, patch, view_image, code_outline, and the `cwd` of
  shell and process_start, which no longer reveal what exists outside the
  root (an empty `cwd` stays the root): Python raises
  `InvalidInputError`, RoomKit refuses the call in those words, and the model
  reads which path to correct. It was `ToolIOError`, which the model read as
  "Tool 'read' failed (ToolIOError)", like a permission failure. Other I/O
  failures keep their class and fixed words.

- `grep`'s `include` without `/` filters file names at any depth, as `rg -g`
  does: `include="*.rs"` finds the Rust files of every directory below `path`,
  where it used to read only `path` itself and answer "no matches". An
  `include` with `/` still matches the path relative to `path`.

- `ask_user`: the agent asks the person one to four questions, with up to four
  options each, and waits for the answers. `AskUserTool` checks the call before
  the backend's new `ask` method answers it; through RoomKit,
  `ask_user_handler(timeout)` (Rust too) serves it with the kit's human input,
  and the host's hook reads the questions with `AskUserTool.read`.

- Through RoomKit, `write`, `edit` and `patch` declare `changes_state`: once one
  is served, the agent's repeat counts start again, so a test rerun after an
  edit is no longer refused as a loop. It takes effect with a RoomKit that has
  the option, and on no other tool: a shell command may change files too, but
  two commands alternating in a loop would then go unnoticed.

- A tool's `input_schema` says only what a model needs to write a valid call. It
  no longer carries `$schema`, the input type's `title` and `description` (the
  tool's own description covers them), integer and float width formats such as
  `uint64`, or `"default": null`. Types, bounds, required names, argument
  descriptions and other defaults are unchanged, and an optional argument stays
  nullable. `output_schema` is unchanged.
- `ProcessSession.detach()` closes a session but leaves its running processes
  running, for a host that exits while one must outlive it, such as a server
  started for a later check: their records are released and their pipes
  handed to drainers that discard what the jobs write and outlive the host.
  `ShellTool.create(keep_background_jobs=True)` (Rust:
  `ShellLimits::keep_background_jobs`) leaves the background jobs of a command
  that exits on its own running, its pipes handed to such drainers when a job
  still holds them; a timeout or a cancellation still stops them.
- A shell stream cut short keeps its most recent bytes, where a command reports
  how it ended, instead of its first and last ones, after an
  `[... N earlier bytes omitted ...]` marker. With an output store, only such a
  stream keeps an archive, holding its output up to the store's bounds (8 MiB
  per stream by default); one kept whole needs none and is released as the
  call ends. At a record quota, a new archive releases its owner's oldest
  finished one, never one a running job or call still writes, rather than
  failing with `archive_limit`; a result without an archive id no longer says
  to use `output_read`.
- A tool's `text` says more of what a model needs to go on. `read`, `list`,
  `glob`, `grep` and `code_outline` end with a line of facts such as
  `[next_offset=4, revision=sha256:...]`, not counted in `max_output_bytes`;
  `write` and `edit` give the new `revision`, `ocr` the image's.
  An empty result says so (`no matches`, `no entries`, `no differences`,
  `no processes`). A command reads `exit_code=0`, with `signal`, `timed_out` and
  `output_complete` only when they say something, and leaves out its empty
  streams. Processes and archives name their state as the structured result
  does (`state=finished, end=exited`, `has_more=true`), with no `Some(...)`. An
  archive's line comes before what the command wrote, and is left out when it
  holds nothing and nothing went wrong.
- Through RoomKit, a host's model reads a tool's `text` alone, where it read
  the whole `{data, text}` envelope again on every later request: a
  `plan_update` gave it 1,245 bytes, of which the text was 94. The `data` goes
  to the host as the call's structured content, `{"data": ...}`: an
  `ON_TOOL_CALL` hook reads it as `event.structured_content`, and the call's
  end event carries it; a handler called outside a RoomKit tool call returns
  the text alone, but the plan, process and output adapters, Python's agent
  adapters and a shell with archives still refuse such a call. Python's and Rust's adapters do so for every tool, the shell
  included, which gave its `data` alone, but `view_image`, whose text and image
  parts are unchanged. Refusals, failures and cancellation are unchanged. A
  host that read the data from the result reads it from the structured
  content; a hook that already split the envelope, as roomkit-code's does,
  finds the text alone and leaves it. An `ON_TOOL_CALL` hook that inspects or
  redacts `event.result` alone no longer sees the data: it is in the call's
  structured copy, which the end event keeps and transports receive, so a
  redacting hook must handle `structured_content` too. Python's plans, docs
  and agents adapters set the whole `{data, text}` as structured content; it
  is now `{"data": ...}`, so a host that read `structured_content["text"]`
  reads the result instead. A Rust handler answers
  `ToolResult::Text` where it answered a `ToolResult::Json` object, so a
  wrapper or hook that matched that object no longer finds it: roomkit-code's
  `Scoped` instructions wrapper, which adds `project_instructions` as a field
  of such an object, now appends the rules to the text instead.
- The RoomKit Rust adapter follows roomkit-rs v0.2.0 (4f271c6), which changes no
  existing adapter's API. A tool it declares names no `tags` for Tool Search. An agent's
  requests also declare the kit's `read_stored_result`, which pages through an
  outcome past `evict_threshold_tokens` that the kit stored.
- The RoomKit Rust adapter adds `ViewImage`, behind its optional `image` Cargo
  feature: the text and image parts of Python's adapter, on that engine's image
  results.
- A RoomKit Rust adapter's I/O failure reads as Python's: the model reads
  `Tool 'x' failed (ToolIOError)` and observers its public words, without the OS
  detail.
- Tool and parameter descriptions are about 45% shorter and keep every rule a
  call needs. Names, types, defaults and behaviour are unchanged.

## 0.1.0a16

### Breaking changes

- Commands from `shell` and the process tools no longer inherit the host's whole
  environment. They receive only the variables named in `inherit_env` (by
  default `DEFAULT_INHERITED_ENV`: `PATH`, `HOME`, `USER`, `LOGNAME`, `SHELL`,
  `LANG`, `LC_ALL`, `LC_CTYPE`, `TZ`, `TMPDIR`) plus the explicit values in
  `env`. Python takes both as keywords on `ShellTool.create` and
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
- The RoomKit Rust adapter follows roomkit-rs 5d88436, whose tools take
  Python's Tool protocol: each adapter implements `roomkit_ai::Tool` and is
  declared as `ToolEntry::Tool(Arc::new(..))` in `AiChannelOptions::tools`. A
  call under another tool's name is left to the next tool instead of refused.
  The adapters have no `close()` any more: dropping a call cancels it, and the
  host closes the `ProcessSession`, `OutputStore` and backends it shares.
  Owner-scoped adapters (process, output, plans, and the shell with an output
  store) refuse a call made outside a RoomKit tool call, as Python's do.
- Rust: the agent tools' `call` takes its input by value and a
  `&CancellationToken`, as every other tool's, and each `AgentBackend` method
  receives that token. A call cancelled before it reaches the backend returns
  `ToolError::Cancelled`.
- The standalone owner, `owner=None` in Python and `None` in Rust, is apart
  from every named owner: a host passing `"default"` no longer reaches its
  jobs, archives or quota. `close_owner` takes `None` for it (Rust
  `Option<&str>`), and Rust's `ObservedProcess::owner` is an `Option<String>`.
- The agent and web search inputs are named after their tools, in Rust and in
  the schemas the model sees: `DelegateTaskInput`, `TaskStatusInput`,
  `CancelTaskInput`, `AgentFollowupInput`, `AgentSendInput` and
  `WebSearchInput` (were `DelegateInput`, `StatusInput`, `CancelInput`,
  `FollowupInput`, `SendInput` and `SearchInput`).
- `process_status` returns `{"processes": [...]}` instead of a bare array, as
  the other list results do; Rust's `ProcessSession::status` returns a
  `ProcessStatusResult`.
- The `timeout_ms` of `code_outline`, `ocr`, `view_image`, `web_search` and
  `web_fetch` has the process tools' type and minimum: Rust's inputs hold an
  `Option<NonZeroU64>` (were `Option<usize>` or `Option<u64>`), the schemas
  tell the model 0 is invalid, and a 0 is refused when the input is read.
  Above the host maximum they refuse the call, as `shell` and
  `process_start` now do.
- `shell` and `process_start` refuse a `timeout_ms` above the host maximum
  with `InvalidInput` instead of capping it. The seven tools that take a
  `timeout_ms` refuse such a value with one message that gives the maximum,
  such as `timeout_ms exceeds the host maximum of 120000 ms`, so the model
  can correct its value.
- `shell` results and process statuses name their `cwd` relative to the
  workspace, `.` for its root, instead of the host's absolute path.
- `docs_list` and `docs_search` page as `list`, `glob` and `grep` do: `offset`
  and `next_offset` count results from one instead of zero, 0 is refused, and
  an offset past the last result is refused (offset 1 on no result is an
  empty page) instead of returning an empty page. `docs_read`'s `offset`
  schema says its minimum, 1, which the call already required. Python's
  `FileDocs.list` and `search` default to `offset=1`, and a host
  `DocsBackend` receives and returns one-based offsets.
- Python: the documentation, plan and agent tools split into a tool and its
  RoomKit adapter, as `ReadTool` and `Read`: `DocsList(backend).call(arguments)`
  is `DocsListTool(backend).call(arguments)`, `DocsList(backend)` in RoomKit's
  tools is `DocsList(DocsListTool(backend))`, and `DocsList.contract()` is
  `DocsListTool.definition()`; likewise `DocsSearch`, `DocsRead`, `PlanCreate`,
  `PlanRead`, `PlanList`, `PlanUpdate`, `PlanDelete`, `DelegateTask`,
  `TaskStatus`, `CancelTask`, `AgentFollowup` and `AgentSend`.
  The adapters now live in `roomkit_tools.roomkit`: `roomkit_tools.docs`,
  `roomkit_tools.plans` and `roomkit_tools.agents` hold the tools only, so
  `from roomkit_tools.docs import DocsList` raises `ImportError`; import
  `roomkit_tools.roomkit.docs.DocsList` (likewise
  `roomkit_tools.roomkit.plans.PlanCreate` and
  `roomkit_tools.roomkit.agents.DelegateTask`), or `roomkit_tools.DocsList`
  as before.
- Python: the RoomKit adapters take a built tool, as Rust's do:
  `Read(root, max_lines=200)` is `Read(await ReadTool.create(root,
  max_lines=200))`, and likewise `List`, `Glob`, `Grep`, `Diff`, `Edit`,
  `Write`, `Patch`, `CodeOutline`, `Shell`, `ViewImage`, `Ocr`
  (`Ocr(await OcrTool.create(root, backend=backend))`), `WebFetch`
  (`WebFetch(WebFetchTool(...))`) and `WebSearch`
  (`WebSearch(WebSearchTool(backend=...))`). A root or limit is checked when
  the tool is built rather than at the first call; `ReadToolAdapter`, which
  took a built `ReadTool`, is `Read`.
  Building an adapter therefore makes a native call, awaited for most tools
  (`await ReadTool.create(...)`), which needs a running event loop. A host
  that forks after building its tools (gunicorn `--preload`, a
  `multiprocessing` pool with the `fork` start method) gets
  `ForkedProcessError` in its workers: build the tools after forking, or use
  the `spawn` start method.
- Python names every limit where it took `**limits` or `**configuration`:
  `FileDocs(root, *, version=None, max_documents=None, ...)`,
  `OcrTool.create(root, *, backend, max_file_bytes=None, ...)`,
  `WebFetchTool.create(*, allowed_origins=None, ...)` and
  `WebSearchTool.create(*, backend, max_query_bytes=None, ...)`, the fields
  of Rust's limits and configuration structs, `None` keeping Rust's default.
  A misspelled limit given to `FileDocs` or `OcrTool.create` now raises
  `TypeError` instead of `InvalidInputError`, as the web and search tools'
  already did.

### Changed

- `web_fetch` converts HTML with the new `htmlmark` crate. Its Markdown differs
  from before in form: pipe tables are not padded to a common column width,
  lists use `- ` and `1. ` markers, emphasis that a CommonMark reader would
  show as literal asterisks is left out (its text stays), and a link nested in
  another keeps only its text. Text format output keeps the same words; it no
  longer resolves link URLs it never shows, so a page is no longer refused in
  text format for the size of its expanded links.
- Rust: the core error is `ToolError`, as in Python, and its variants
  `Refused` and `Unsupported` are `AccessRefused` and `UnsupportedResource`,
  as Python's `AccessRefusedError` and `UnsupportedResourceError`.
- Rust takes Python's names: `ProcessManager` is `ProcessSession`,
  `MutationWorkspace` is `Workspace`, `RustoBackend` is `RustoOcr`, and
  `OutputStore::is_closed` is `closed`. `ProcessSession::closed` reports a
  requested shutdown, as Python's `closed` property.
- Rust: `ProcessSession` (`start`, `status`, `read`, `write`, `stop`) and
  `OutputStore` (`read`, `forget`, `list`) take `owner: Option<&str>`; `None`
  is the standalone owner, the one `ShellTool::shell` files archives under,
  as Python's `owner=None`.
- Rust: `OutputStore::list` is async, as Python's `OutputStore.list`.
- Rust RoomKit adapter: the process adapters take `session` and
  `OutputRead` and `OutputForget` take `store`, as Python's
  `ProcessStart(session)` and `OutputRead(store)`.
- Rust: the limits, policy and configuration structs a host builds are no
  longer `#[non_exhaustive]`, so `ReadLimits { max_lines: 200,
  ..ReadLimits::default() }` sets some fields as Python's keywords do. A
  literal naming every field now breaks when a field is added.
- Rust takes Python's defaults: `ListInput::default()` lists the root;
  `ProcessWriteInput`, `ProcessStopInput`, `OutputReadInput`, `OcrInput`,
  `DocsSearchInput`, `DocsReadInput` and `PlanUpdateInput` gain `new` with
  their required fields, and `DocsListInput` a `Default`, the rest defaulted
  as in Python; `ViewImageInput::new(path)` replaces its `Default`, the path
  being required. `FileDocs::new(root, limits)` takes the version from
  `with_version`, `ExaSearch::new(config)` the key from `with_api_key`,
  `MemoryPlanBackend::new` a `MemoryPlanLimits` (256 and 64 by default), and
  `OutputStore::new` an optional parent, the system temporary directory for
  `None`.
- Rust takes flat what Python takes flat: `PatchLimits` holds the file and
  comparison bounds itself instead of a nested `DiffLimits`,
  `SearchHttpConfig` holds `allowed_origins` and `private_origins` instead of
  a `NetworkPolicy`, which `roomkit_tools_search` no longer exports, and
  `SearchResult` holds `payload`, `partial`, `notices` and `usage`, its JSON
  unchanged, instead of a `BackendOutput`.
- Rust: `RustoConfig` takes the models directory holding `det.onnx`,
  `rec.onnx` and `dict.txt`, as Python's `RustoOcr(models)`, instead of the
  three paths; `RustoConfig::new(models)` defaults `model_name`
  (`ppocrv6`), `threads` (2) and `detection_max_edge` (960).
- Rust: `OcrTool::new(root, backend)` names its engine `backend`, as
  Python's `OcrTool.create(root, backend=...)`. The OCR backend protocol
  itself still differs for this release (Python's `OcrBackend.bind`, Rust's
  `OcrBackend::recognize` in an `OcrEngine`); the tool and its contract are
  the same.
- Rust: `ImageMime::as_str` and `ImagePayload::data_url` leave the documented
  API, as Python reads the same JSON; they stay callable for the crates that
  share them.
- Rust RoomKit adapter: each adapter takes the name of its Python RoomKit
  tool, `<X>Toolset` becoming `<X>`: `ReadToolset::new(tool)` is
  `Read::new(tool)`, and likewise `List`, `Glob`, `Grep`, `Diff`, `Edit`,
  `Write`, `Patch`, `CodeOutline`, `Shell`, `Ocr`, `WebFetch`, `WebSearch`,
  `ProcessStart`, `ProcessStatus`, `ProcessRead`, `ProcessWrite`,
  `ProcessStop`, `OutputRead`, `OutputForget`, `DocsList`, `DocsSearch`,
  `DocsRead`, `PlanCreate`, `PlanRead`, `PlanList`, `PlanUpdate` and
  `PlanDelete`. Name `Read` and `Write` by path where `std::io`'s are in
  scope.
- Rust: the documentation and plan tools' `execute` is `call`, as Python's:
  `DocsListTool::call(input, &cancel)`, `PlanCreateTool::call(&scope, input,
  &cancel)`.
- Rust: the plumbing the tool crates share leaves the public API, hidden from
  the documentation: `roomkit-tools-http` is internal, as are core's
  `WorkspaceRoot`, `identifiers`, `revision`, `Output::new` and
  `Definition::new`, the documentation, plan and agent crates' `decode`,
  `render` and contract traits, the scopes' and `CommandEnv`'s `validate`,
  and the image crate's `ImageWorkspace` and `decode_rgb`.
  `FileDocsLimits::validate` is private. `WebFetchPolicy` is now the web
  crate's own type, with the same fields.

### Added

- Python: the `WebSearchTool.capabilities` property reports the selected
  backend's result kinds and service limits, as the backend's own property
  and Rust's `WebSearchTool::capabilities()` do.
- `web_fetch` uses [`htmlmark`](https://crates.io/crates/htmlmark) 0.1:
  bounded, interruptible HTML to Markdown and text conversion for untrusted
  pages, on html5ever with its own tree. It was written here and is now
  maintained in its own repository. A property test reads its Markdown back
  with a CommonMark parser and checks the page's text and links survive
  exactly, and two fuzz targets check its limits and the same property.
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

### Fixed

- `web_fetch` checks `max_converted_bytes` on every write of the converted
  text. It no longer refuses a page whose Markdown fits the limit but whose
  estimated size did not, and cancellation or the deadline now also stops
  Markdown rendering under way.
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
- A `ShellTool` keeps the environment options `ShellTool.create` validated,
  and the RoomKit tools refuse room or channel ids that are not Unicode text.
- `ViewImageTool.create`, `CodeOutlineTool.create` and `OcrTool.create` take
  their limits as explicit keywords, so a misspelled option fails when the
  tool is built.
- Documentation tools let I/O errors propagate in RoomKit like the other tools.
- Tool exceptions report `roomkit_tools` as their module and can be pickled;
  native contract classes report `roomkit_tools._native`.

### Packaging and documentation

- The RustO OCR crate requires `rusto-rs` 0.3.2 or a compatible later
  release instead of exactly 0.3.2; `Cargo.lock` keeps 0.3.2.
- Drop `htmd` and `markup5ever_rcdom`, html5ever's test DOM that its authors
  do not support for untrusted input; update html5ever to 0.40.
- Update to sha2 0.11, base64 0.23 and reqwest 0.13. HTTPS keeps the ring
  provider and the bundled Mozilla roots, now configured explicitly, so
  certificate trust does not depend on the host's certificate store.
- The wheel ships the license notices of every native dependency
  (`licenses/native_dependencies.txt`), checked against the lock file.
- Distribution checks admit only tracked files in source archives and expected
  paths in wheels.
- The guides state that stopping a process sends SIGKILL to its group, what
  replacing a file by rename does not keep, and the measured peak memory of
  `code_outline` and `view_image`.
- Align the optional Rust adapter with the current RoomKit engine revision,
  retaining individually selected tools and host-owned lifecycles.
- The Rust library crates are ready for crates.io: each has a README, its
  license text, keywords and categories, and `cargo publish --workspace
  --dry-run` packages and builds them. Each README but the internal HTTP
  crate's has an example compiled as a doctest; it also runs, except the web,
  search and RustO ones, which need the network or local models. The
  examples, the Python native modules and the RoomKit adapter stay
  unpublished.
- The Rust library crates are published to crates.io as `0.1.0-alpha.16`. A
  pre-release matches no plain `"0.1"` requirement: depend on that exact
  version, such as `roomkit-tools-fs = "0.1.0-alpha.16"`. The RoomKit Rust
  adapter stays unpublished and is used through Git.


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
