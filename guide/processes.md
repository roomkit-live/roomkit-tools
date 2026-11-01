# Commands, processes and output archives

Python snippets with `await` belong inside an async function. RoomKit fragments
assume an existing configured provider.

## Foreground commands

Execute a foreground command:

```python
from roomkit_tools import ShellTool

shell = await ShellTool.create("./workspace")
result = await shell.shell("printf ready; printf warning >&2; exit 7", timeout_ms=1000)
print(result["data"]["stdout"]["text"])  # ready
print(result["data"]["stderr"]["text"])  # warning
print(result["data"]["exit_code"])       # 7: a command result, not a tool error.
```

`Shell(root=...)` is the direct RoomKit form. The standalone method is
`ShellTool.shell(command, cwd=None, timeout_ms=None)`; `.call(arguments)` accepts
the Rust schema. The root and per-call cwd select the initial working directory.
Commands run with `/bin/sh -c`, inherit the Python host's environment and
permissions, and can access paths outside the root. This is not a sandbox.
Passing a `Workspace` supplies its path only; shell writes do not share the
filesystem mutation lock. There is no login shell, PTY, interactive stdin or
persistent session; stdin receives EOF. Each command leads a new Unix session
with no controlling terminal: a program that opens `/dev/tty` gets an error
instead of writing into the host's terminal or waiting on it.

Host constructor bounds are `max_command_bytes` (16 KiB), `max_output_bytes`
(32 KiB raw bytes per stream), `default_timeout_ms` (10,000) and `max_timeout_ms`
(120,000). All must be positive; default timeout cannot exceed the maximum,
and the maximum cannot exceed 24 hours. A requested runtime is capped by the
host. It starts after spawn, excluding path resolution; termination/reaping
may extend total elapsed time.

The result preserves separate stdout/stderr, exit code or signal, effective
timeout and `timed_out`. Each stream retains head/tail output while continuing
to drain, with `total_bytes`, `omitted_bytes` and `truncated`. Text uses lossy
UTF-8; replacement characters, omission markers and metadata are additional to
the raw retention bound. `output_complete` reports whether both pipes reached
EOF; false means capture stopped early. Nonzero exits and timeouts are normal
tool results; timeouts retain the output captured before termination.

Timeout, cancellation and normal shell exit stop the Unix process group and
reap the direct child. Processes that deliberately leave that group are outside
this cleanup mechanism. Cancelling a Python task raises `CancelledError` and
requests cleanup in the owned Rust worker; cleanup can finish after that
exception is delivered. Keep the host process/native runtime alive for cleanup;
already-performed effects are not rolled back. Linux is validated. The existing
Rust implementation returns an unsupported error on non-Unix platforms.
See [the standalone shell example](../examples/run_shell.py):
`.venv/bin/python examples/run_shell.py`.

## Persistent processes

For commands that must remain alive across calls, share an explicit session:

```python
from roomkit import AIChannel
from roomkit_tools import (
    ProcessSession, ProcessStart, ProcessStatus, ProcessRead, ProcessWrite, ProcessStop,
)

# Inside an async function; keep this scope alive for the conversation.
async with ProcessSession(root="./workspace") as processes:
    agent = AIChannel(
        "agent",
        provider=provider,
        tools=[
            ProcessStart(processes), ProcessStatus(processes), ProcessRead(processes),
            ProcessWrite(processes), ProcessStop(processes),
        ],
    )
    # Register the agent and run the conversation here, closing RoomKit before
    # leaving this scope. Only the selected tools are exposed.
```

`ProcessSession` owns a distinct Rust manager with one host-selected owner.
Create one session per conversation/agent requiring isolation: the Python Tool
handler does not receive a trusted conversation identity. Sharing a session
deliberately shares all its jobs. IDs such as `p1` are local record identifiers,
not OS PIDs or secrets, and can coincide across sessions. There is no global
registry or cross-session quota. Passing a `Workspace` supplies only its path;
processes do not acquire the filesystem mutation lock or run in a sandbox.

Standalone callers use `await processes.start("cat")`, `.status(process_id)`,
`.read(process_id, stdout_offset=0, stderr_offset=0, max_bytes=None)`,
`.write(process_id, "Hello\n", close_stdin=False)` and `.stop(process_id, forget=True)`.
All return the existing Rust `data`/`text` dictionaries. `.call(name, arguments)`
and `.definition(name)` expose each `process_*` contract and its derived schemas.
`status()` lists retained jobs, including completed ones. `start` returns an ID
with open stdin, not proof of command completion. Commands use `/bin/sh -c`
with the same host permissions, environment, initial-cwd boundary and new
session without a terminal as shell.

`read` immediately observes available output without consuming it or sending
EOF. Use each stream's `next_offset` separately. Empty output while running
means nothing new is available yet. The journal retains the most recent bytes;
`omitted_bytes` reports data already evicted. Concatenate the raw `bytes` arrays
before decoding UTF-8 split across pages; each page's `text` is a lossy view.
`write` sends exactly the supplied UTF-8 bytes, including only explicitly supplied
newlines. Inspect `complete`, `bytes_written`, `stdin_closed` and `error`: timeout,
cancellation or exit can leave a partial write. Accepted bytes do not prove the
child consumed them. Send EOF explicitly with `close_stdin=True` when requested.

| Session constructor option | Default |
| --- | --- |
| `max_processes` / `max_processes_per_owner` | 32 / 8 retained jobs; both apply to the session's single owner |
| `max_command_bytes` | 16 KiB UTF-8 |
| `max_output_bytes` | 1 MiB recent raw bytes per stream per job |
| `max_read_bytes` | 16 KiB per stream per call |
| `max_write_bytes` | 64 KiB UTF-8 per call |
| `write_timeout_ms` | 1,000 ms |
| `default_timeout_ms` / `max_timeout_ms` | 300,000 / 3,600,000 ms process lifetime |

All bounds must be positive. Per-owner quota cannot exceed total quota, the
write deadline cannot exceed 60 seconds, and default lifetime cannot exceed
the maximum (at most 24 hours). A requested lifetime is capped by the host.
Completed jobs retain output and consume quota until `stop(..., forget=True)`
or session shutdown. `stop` targets the Unix process group, waits up to two
seconds and reports `forgotten=False` if cleanup is still pending.

`await processes.close()` permanently closes the session, stops owned jobs and
waits up to two seconds: `True` means cleanup finished and records were released;
`False` means cleanup continues and the host can await `close()` again. Context
exit calls it and raises `ToolError` if cleanup remains pending. `.closed` means
shutdown was requested, not necessarily finished. The last session/tool reference
also requests cleanup on drop; prefer explicit context management and keep the
host/native runtime alive for cleanup. Escaped process groups and abrupt host
termination remain outside the cleanup guarantee.

Cancelling `read`, `status` or `write` does not terminate the job. A cancelled
start may have already spawned; recover its record with `status()` before retrying.
A cancelled write may have delivered bytes; do not blindly replay it. Once
issued, stop and close continue even if their Python awaiter is cancelled.
See [the standalone process example](../examples/run_process.py), which verifies
exact newline bytes, paged reads with open stdin, explicit EOF and normal exit:
`.venv/bin/python examples/run_process.py`.

## Optional command output retention

Pass one host-owned `OutputStore` to the command tools that need disk copies.
Select `OutputRead` to let the agent retrieve bytes omitted from the memory preview;
select `OutputForget` only if the agent should also release completed logs.

```python
from roomkit import AIChannel, RoomKit
from roomkit_tools import OutputStore, OutputRead, OutputForget, Shell

async with OutputStore() as outputs, RoomKit() as kit:
    agent = AIChannel(
        "agent",
        provider=provider,
        tools=[
            Shell("./workspace", outputs=outputs),
            OutputRead(outputs),
            OutputForget(outputs),
        ],
    )
    kit.register_channel(agent)
    # Create/attach the room and run the conversation here.
```

For persistent jobs, use `async with ProcessSession(root, outputs=outputs)` inside
that store's context, then select the desired `Process*` tools. Close RoomKit and
the process session before the output store. Python hosts must create a separate
store per conversation/agent requiring isolation; sharing a store intentionally
shares access to its logs. Rust hosts supply trusted owner keys; Rust RoomKit
adapters derive them from the room and agent. Owners and storage paths are not
model arguments.

Standalone Python uses the same Rust implementation:

```python
from roomkit_tools import OutputStore, ShellTool

async with OutputStore() as outputs:
    shell = await ShellTool.create("./workspace", outputs=outputs, max_output_bytes=16)
    result = await shell.shell("printf 'a longer diagnostic message'")
    archive = result["data"]["stdout"]["archive"]
    if archive["archive_id"] is not None:
        page = await outputs.read(archive["archive_id"], offset=0, max_bytes=4096)
        print(page["data"]["text"])
        print(page["data"]["archive"]["complete"])
        await outputs.forget(archive["archive_id"])
```

Without `outputs=`, command result shapes and memory retention are unchanged.
With it, each stream has archive metadata: `archive_id`, `observed_bytes`,
`stored_bytes`, `eof`, `finished`, `complete` and `error`. The process forms are
`stdout_archive` and `stderr_archive` on its status. IDs identify streams in that
store, never arbitrary host files. stdout and stderr are separate; their global
ordering is not reconstructed. An ID may be absent when the record quota is full.

`complete` means the source reached EOF, the writer finished without errors,
and every observed byte was stored. It does not mean the command succeeded or
captured every possible descendant output: check command status and
`output_complete` separately. A timeout can still leave a complete copy of the
bytes captured before termination. An incomplete disk copy is a prefix, with
an explicit error such as `stream_limit`, `total_limit` or `write_failed`.

`output_read` returns raw `bytes`, display `text`, byte `offset`/`next_offset`,
`has_more` and the archive snapshot. Reads neither consume data, wait for more,
rerun commands nor close stdin. `has_more: false` describes only the current
stored prefix; it can grow while capture/writing continues. Concatenate raw
bytes before decoding Unicode; display text replaces invalid/split sequences.
Offsets past the stored prefix are invalid. Requests are capped by the host.
The JSON array and lossy text add representation overhead to the raw page bound.

| Host option | Default |
| --- | --- |
| `max_archives` / `max_archives_per_owner` | 64 / 16 retained streams, including empty and failed copies |
| `max_stream_bytes` | 8 MiB retained prefix per stream |
| `max_total_bytes` | 64 MiB shared stored/reserved bytes |
| `max_read_bytes` | 16 KiB raw bytes per page |
| `write_timeout_ms` | 1,000 ms to wait for queue capacity or final flush; maximum 60,000 ms |

All bounds are positive. A command normally creates two records. The bounded
writer queue holds at most eight 8 KiB chunks per stream, plus pending/in-flight
chunks. Disk I/O runs outside journal locks and async executor threads. Waiting
for queue capacity can slow capture up to the configured deadline; command
timeouts still apply. Quota, file creation, write or queue errors disable copying
with explicit metadata while pipe draining and bounded previews continue.
Reservations include queued bytes and stay charged after failed writes until
forget/close. There is no silent eviction or unlimited queue.

Completed logs survive process `forget` and `ProcessSession.close()`.
`output_forget` refuses active capture/writers, then invalidates the ID and frees
its reservation. The host's `await outputs.list()` recovers IDs after a cancelled
command response. `await outputs.close()` permanently refuses reads and new
records, stops writers and releases retained files; it does not stop commands.
It waits up to two seconds for writers; `False` means cleanup needs another
close attempt. An issued close continues if its Python awaiter is cancelled.
Already-issued filesystem work can outlive cancellation; last-reference cleanup
only requests shutdown. Keep the host runtime alive for explicit cleanup.

The store creates a private temporary directory (mode 0700) beneath an optional
host `parent` (default: system temporary directory in Python), using anonymous
files on Unix.
It is ephemeral: no fsync durability, restart recovery or export API is promised.
Linux is validated; this does not add a sandbox or PTY.

Run [the RoomKit log example](../examples/roomkit_output.py) with the installed package:

```sh
python examples/roomkit_output.py
# With OPENAI_BASE_URL and OPENAI_MODEL already configured:
python examples/roomkit_output.py --live
```

It verifies one real shell execution, a truncated preview and exact recovery of
a middle diagnostic through `output_read`. The default model is scripted; the
RoomKit loop, native tools, disk reads and verifier are real.

## Rust host observation

Subscribe with `ProcessManager::subscribe()` before calling `observe()`. The watch
receiver coalesces starts, completions, removal and shutdown; output chunks do not
produce notifications. Observations include the exact command, trusted owner key,
elapsed runtime (frozen on completion) and current status. This is a privileged
host API: filter owners before displaying jobs to a user. It does not change the
owner-scoped model tool contracts. Use bounded `read()` pages when a view is open,
and drop the watch receiver when the view disconnects.

[Documentation index](README.md) · [Project overview](../README.md)
