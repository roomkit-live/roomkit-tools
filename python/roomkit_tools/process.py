"""Async command execution backed by roomkit-tools-process."""

from __future__ import annotations

import json
import os
from collections.abc import AsyncIterator, Mapping, Sequence
from types import TracebackType
from typing import Any, Self

from . import _native
from ._ends import _JobEnds
from ._shell import DEFAULT_INHERITED_ENV, ShellTool, _env_options
from .output import OutputStore

__all__ = ["DEFAULT_INHERITED_ENV", "ProcessSession", "ShellTool"]


class ProcessSession:
    """A host-owned collection of persistent processes for one conversation/agent.

    Use ``async with ProcessSession(root) as session`` for bounded cleanup on
    exit. Construction captures an absolute root; first use resolves it in Rust.
    Sessions are independent, each with its own jobs. Every job belongs to one
    owner, a trusted host string: the RoomKit process tools give each room and
    acting agent its own owner, roomkit_owner(room_id, channel_id), so one
    session can serve several conversations without them seeing each other's
    jobs. The host methods take owner=; None, the default, is one standalone
    owner, apart from every named owner, "default" included. IDs are local to
    this session and sequential, not OS PIDs, credentials or portable
    identifiers.

    Commands use /bin/sh -c with piped stdin, the host's permissions, the
    environment selected by inherit_env and env (as for ShellTool), a new
    session without a controlling terminal, and no PTY or sandbox.
    Workspace is accepted as a path only. Closing stops process groups; escaped
    groups and already-performed effects are not undone.
    """

    def __init__(
        self,
        root: str | os.PathLike[str],
        *,
        outputs: OutputStore | None = None,
        inherit_env: Sequence[str] = DEFAULT_INHERITED_ENV,
        env: Mapping[str, str] | None = None,
        max_processes: int | None = None,
        max_processes_per_owner: int | None = None,
        max_command_bytes: int | None = None,
        max_output_bytes: int | None = None,
        max_read_bytes: int | None = None,
        max_write_bytes: int | None = None,
        write_timeout_ms: int | None = None,
        default_timeout_ms: int | None = None,
        max_timeout_ms: int | None = None,
        max_wait_ms: int | None = None,
    ) -> None:
        """Configure the command environment and Rust bounds.

        inherit_env and env select the environment as for ShellTool.create and
        are validated here. Defaults: 32 total/8 per owner, 16 KiB command,
        1 MiB retained bytes per stream, 16 KiB/read/stream, 64 KiB/write, 1 s
        stdin deadline, 5 min default and 1 h maximum lifetime, 20 s longest read
        wait, below RoomKit's 30 s default tool timeout. The per-owner
        quota applies to the standalone owner and to each room and agent; the
        total quota to the whole session. Neither is global across sessions.
        All bounds must be positive, per-owner quota <= total quota, stdin
        deadline <= 60 s, default lifetime <= maximum lifetime <= 24 h, and
        longest read wait <= 24 h.
        """
        path = os.fspath(root)
        if outputs is not None and not isinstance(outputs, OutputStore):
            raise TypeError("outputs requires an OutputStore")
        self._outputs = outputs
        if not path:
            raise _native.InvalidInputError("root must not be empty")
        environment = _env_options(inherit_env, env)
        options = {
            name: value
            for name, value in {
                "max_processes": max_processes,
                "max_processes_per_owner": max_processes_per_owner,
                "max_command_bytes": max_command_bytes,
                "max_output_bytes": max_output_bytes,
                "max_read_bytes": max_read_bytes,
                "max_write_bytes": max_write_bytes,
                "write_timeout_ms": write_timeout_ms,
                "default_timeout_ms": default_timeout_ms,
                "max_timeout_ms": max_timeout_ms,
                "max_wait_ms": max_wait_ms,
            }.items()
            if value is not None
        } | environment
        self._native = _native._ProcessSession(
            os.path.abspath(path),
            json.dumps(options, allow_nan=False),
            None if outputs is None else outputs._native,
        )

    @property
    def closed(self) -> bool:
        """Whether shutdown was requested, not whether all cleanup has finished."""
        return self._native.is_closed()

    async def __aenter__(self) -> Self:
        cancel = _native._Cancellation()
        try:
            await self._native.initialize(cancel)
        finally:
            cancel.cancel()
        return self

    async def __aexit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        if not await self.close():
            raise _native.ToolError("Process cleanup is still pending; await session.close() again")

    async def close(self) -> bool:
        """Permanently close the session and wait up to two seconds for cleanup.

        True means cleanup finished and records were released. False means
        workers continue; call close again to await them. Repeated close is safe.
        Once this coroutine starts, shutdown continues even if its awaiter is
        cancelled. Context exit raises ToolError if cleanup is still pending.
        Dropping the last session/tool reference also requests cleanup, but
        explicit close is preferred. Keep the host/native runtime alive for it.
        """
        return await self._native.close()

    async def detach(self) -> bool:
        """Permanently close the session but leave its running processes running.

        For a host that exits while a process must outlive it, such as a server
        started for a later check. Their records and retained output are
        released, and their output pipes go to drainers that discard what they
        write and outlive the host, so their writes never fail.
        Finished jobs are released as by close. True means every job was let go
        within two seconds; False means workers continue, as for close.
        """
        return await self._native.detach()

    def ends(self) -> AsyncIterator[dict[str, Any]]:
        """Follow the jobs that finish from now on, whatever their owner.

        ``async for end in session.ends()`` gives one dict per job that
        exited, timed out, was stopped or failed: owner, tool ("process"),
        process_id, command, cwd, end, exit_code, signal and elapsed_ms. Call
        it before starting the jobs to follow; an end before the call is not
        told. A detached job is let go, not ended, and is not told. The
        iteration stops once the session closes. owner is the host's key:
        filter by it before telling another principal.
        """
        return _JobEnds(self._native.ends())

    async def close_owner(self, owner: str | None) -> bool:
        """Stop one owner's jobs and release their records; the session stays open.

        None names the standalone owner, apart from every named owner.

        Waits up to two seconds, as close does: False means cleanup continues;
        call close_owner again to await it. The owner can start jobs again
        afterwards, and other owners' jobs are untouched. Once this coroutine
        starts, cleanup continues even if its awaiter is cancelled.
        """
        return await self._native.close_owner(owner)

    @staticmethod
    def definition(name: str) -> dict[str, Any]:
        """Return one of the five process_* definitions and its Rust-derived schemas."""
        return json.loads(_native._ProcessSession.definition_json(name))

    async def call(
        self, name: str, arguments: dict[str, Any], *, owner: str | None = None
    ) -> dict[str, Any]:
        """Execute one process_* operation with Rust validation and data/text output.

        owner names whose jobs the operation reaches: a host string of 1 to
        1024 UTF-8 bytes, never a model argument; None is the standalone owner.
        Cancelling an observation or write does not stop the process. Once
        start has spawned, a cancelled Python caller may lose the returned ID:
        recover it with status(), rather than blindly starting again. A cancelled
        write may have delivered bytes; do not blindly replay. Once requested,
        stop continues even if its waiter is cancelled. Shutdown is explicit.
        """
        cancel = _native._Cancellation()
        try:
            arguments_json = json.dumps(arguments, allow_nan=False)
            result = await self._native.call(name, arguments_json, cancel, owner)
        finally:
            cancel.cancel()
        return json.loads(result)

    async def start(
        self,
        command: str,
        *,
        cwd: str | os.PathLike[str] | None = None,
        timeout_ms: int | None = None,
        owner: str | None = None,
    ) -> dict[str, Any]:
        """Start with open stdin and return an ID, not proof of command completion."""
        return await self.call(
            "process_start",
            {
                "command": command,
                "cwd": None if cwd is None else os.fspath(cwd),
                "timeout_ms": timeout_ms,
            },
            owner=owner,
        )

    async def status(
        self, process_id: str | None = None, *, owner: str | None = None
    ) -> dict[str, Any]:
        """Inspect one ID or list the owner's retained jobs, including completed ones.

        Lists in creation order. Jobs started through the RoomKit tools belong
        to their room and agent: pass owner=roomkit_owner(room_id, channel_id).
        """
        return await self.call("process_status", {"process_id": process_id}, owner=owner)

    async def read(
        self,
        process_id: str,
        *,
        stdout_offset: int = 0,
        stderr_offset: int = 0,
        max_bytes: int | None = None,
        wait_ms: int | None = None,
        owner: str | None = None,
    ) -> dict[str, Any]:
        """Read available bytes without writing, consuming output or EOF.

        Use independent next_offset cursors; omitted_bytes reports retention
        gaps. Join the raw bytes arrays before decoding split UTF-8/binary pages.
        Empty output while running means nothing new yet. With wait_ms, the read
        first waits up to that many ms, capped by max_wait_ms, for the process
        to end or to write past the offsets.
        """
        return await self.call(
            "process_read",
            {
                "process_id": process_id,
                "stdout_offset": stdout_offset,
                "stderr_offset": stderr_offset,
                "max_bytes": max_bytes,
                "wait_ms": wait_ms,
            },
            owner=owner,
        )

    async def write(
        self,
        process_id: str,
        text: str = "",
        *,
        close_stdin: bool = False,
        owner: str | None = None,
    ) -> dict[str, Any]:
        """Send exact UTF-8 text; no newline is added. EOF is explicit.

        Inspect complete/bytes_written/error: pipe writes may be partial on
        timeout, cancellation or exit. Accepted bytes need not be consumed by
        the child. Closing stdin may end the job; reading never requires it.
        """
        return await self.call(
            "process_write",
            {
                "process_id": process_id,
                "text": text,
                "close_stdin": close_stdin,
            },
            owner=owner,
        )

    async def stop(
        self, process_id: str, *, forget: bool = False, owner: str | None = None
    ) -> dict[str, Any]:
        """Send SIGKILL to the process group and wait at most two seconds for cleanup.

        There is no graceful phase: the command cannot save state or clean up.
        forget=True releases completed records/output and invalidates the ID.
        If cleanup is still running, forgotten remains false; inspect status.
        """
        return await self.call(
            "process_stop", {"process_id": process_id, "forget": forget}, owner=owner
        )
