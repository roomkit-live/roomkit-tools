"""Async command execution backed by roomkit-tools-process."""

from __future__ import annotations

import json
import os
from types import TracebackType
from typing import Any, Self

from . import _native
from .output import OutputStore


class ShellTool:
    """Run foreground commands using /bin/sh -c, with separate bounded streams.

    Commands inherit the host environment and permissions. The root bounds only
    the initial cwd, not filesystem/network access. No sandbox, mutation lock,
    login shell, PTY, interactive stdin or persistent session is provided.
    Stdin is closed. Each command leads a new Unix session without a controlling
    terminal, so opening /dev/tty fails. Unix is implemented; Linux is validated.
    """

    def __init__(self, native: _native._ShellTool, outputs: OutputStore | None = None) -> None:
        self._native = native
        self._outputs = outputs

    @classmethod
    async def create(
        cls,
        root: str | os.PathLike[str],
        *,
        outputs: OutputStore | None = None,
        max_command_bytes: int | None = None,
        max_output_bytes: int | None = None,
        default_timeout_ms: int | None = None,
        max_timeout_ms: int | None = None,
    ) -> ShellTool:
        """Resolve an existing root; omitted limits use Rust defaults.

        Defaults: 16 KiB command, 32 KiB retained raw bytes per stream, 10 s
        default runtime and 120 s maximum. Bounds must be positive; the default
        timeout must not exceed the maximum, which cannot exceed 24 hours.
        """
        if outputs is not None and not isinstance(outputs, OutputStore):
            raise TypeError("outputs requires an OutputStore")
        options = {
            name: value
            for name, value in {
                "max_command_bytes": max_command_bytes,
                "max_output_bytes": max_output_bytes,
                "default_timeout_ms": default_timeout_ms,
                "max_timeout_ms": max_timeout_ms,
            }.items()
            if value is not None
        }
        cancel = _native._Cancellation()
        try:
            native = await _native._ShellTool.create(
                os.fspath(root),
                json.dumps(options, allow_nan=False),
                cancel,
                None if outputs is None else outputs._native,
            )
        finally:
            cancel.cancel()
        return cls(native, outputs)

    @staticmethod
    def definition() -> dict[str, Any]:
        """Return fresh input/output schemas derived from the Rust types."""
        return json.loads(_native._ShellTool.definition_json())

    async def shell(
        self,
        command: str,
        *,
        cwd: str | os.PathLike[str] | None = None,
        timeout_ms: int | None = None,
    ) -> dict[str, Any]:
        """Run with a per-call cwd and runtime capped by the host maximum.

        Nonzero exits and timeouts are results, not tool exceptions. Streams
        retain head/tail bytes, with omission metadata and lossy UTF-8 text.
        A timeout returns partial output; output_complete says whether pipes
        reached EOF. Runtime starts after spawn, excluding path resolution;
        termination/reaping can extend wall-clock duration beyond the timeout.
        """
        return await self.call(
            {
                "command": command,
                "cwd": None if cwd is None else os.fspath(cwd),
                "timeout_ms": timeout_ms,
            }
        )

    async def call(self, arguments: dict[str, Any]) -> dict[str, Any]:
        """Execute Rust-validated input and return its data/text envelope.

        Cancelling the Python task raises CancelledError and requests cleanup
        from the owned Rust worker. It may finish cleanup after Python receives
        cancellation. Keep the host process/native runtime alive for cleanup.
        The shell's process group is stopped on cancellation, timeout and normal
        exit; deliberately detached processes and members under other credentials
        are outside that mechanism, and the result is still returned.
        Already-performed effects are not rolled back.
        """
        cancel = _native._Cancellation()
        try:
            result = await self._native.call(json.dumps(arguments, allow_nan=False), cancel)
        finally:
            cancel.cancel()
        return json.loads(result)


class ProcessSession:
    """A host-owned collection of persistent processes for one conversation/agent.

    Use ``async with ProcessSession(root) as session`` for bounded cleanup on
    exit. Construction captures an absolute root; first use resolves it in Rust.
    Each session owns a separate manager and one trusted owner. Never reuse it
    across conversations that must be isolated. IDs are local to this session,
    not OS PIDs, credentials or portable identifiers.

    Commands use /bin/sh -c with piped stdin, inherited environment/permissions,
    a new session without a controlling terminal, and no PTY or sandbox.
    Workspace is accepted as a path only. Closing stops process groups; escaped
    groups and already-performed effects are not undone.
    """

    def __init__(
        self,
        root: str | os.PathLike[str],
        *,
        outputs: OutputStore | None = None,
        max_processes: int | None = None,
        max_processes_per_owner: int | None = None,
        max_command_bytes: int | None = None,
        max_output_bytes: int | None = None,
        max_read_bytes: int | None = None,
        max_write_bytes: int | None = None,
        write_timeout_ms: int | None = None,
        default_timeout_ms: int | None = None,
        max_timeout_ms: int | None = None,
    ) -> None:
        """Configure Rust bounds, including retained completed processes.

        Defaults: 32 total/8 per owner, 16 KiB command, 1 MiB retained bytes per
        stream, 16 KiB/read/stream, 64 KiB/write, 1 s stdin deadline, 5 min default
        and 1 h maximum lifetime. This session has one owner, so both quotas
        apply to its records; neither quota is global across Python sessions.
        All bounds must be positive, per-owner quota <= total quota, stdin
        deadline <= 60 s, and default lifetime <= maximum lifetime <= 24 h.
        """
        path = os.fspath(root)
        if outputs is not None and not isinstance(outputs, OutputStore):
            raise TypeError("outputs requires an OutputStore")
        self._outputs = outputs
        if not path:
            raise _native.InvalidInputError("root must not be empty")
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
            }.items()
            if value is not None
        }
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

    @staticmethod
    def definition(name: str) -> dict[str, Any]:
        """Return one of the five process_* definitions and its Rust-derived schemas."""
        return json.loads(_native._ProcessSession.definition_json(name))

    async def call(self, name: str, arguments: dict[str, Any]) -> dict[str, Any]:
        """Execute one process_* operation with Rust validation and data/text output.

        Cancelling an observation or write does not stop the process. Once
        start has spawned, a cancelled Python caller may lose the returned ID:
        recover it with status(), rather than blindly starting again. A cancelled
        write may have delivered bytes; do not blindly replay. Once requested,
        stop continues even if its waiter is cancelled. Shutdown is explicit.
        """
        cancel = _native._Cancellation()
        try:
            result = await self._native.call(name, json.dumps(arguments, allow_nan=False), cancel)
        finally:
            cancel.cancel()
        return json.loads(result)

    async def start(
        self,
        command: str,
        *,
        cwd: str | os.PathLike[str] | None = None,
        timeout_ms: int | None = None,
    ) -> dict[str, Any]:
        """Start with open stdin and return an ID, not proof of command completion."""
        return await self.call(
            "process_start",
            {
                "command": command,
                "cwd": None if cwd is None else os.fspath(cwd),
                "timeout_ms": timeout_ms,
            },
        )

    async def status(self, process_id: str | None = None) -> dict[str, Any]:
        """Inspect one ID or list all retained jobs, including completed ones."""
        return await self.call("process_status", {"process_id": process_id})

    async def read(
        self,
        process_id: str,
        *,
        stdout_offset: int = 0,
        stderr_offset: int = 0,
        max_bytes: int | None = None,
    ) -> dict[str, Any]:
        """Read currently available bytes without writing, consuming output or EOF.

        Use independent next_offset cursors; omitted_bytes reports retention
        gaps. Join the raw bytes arrays before decoding split UTF-8/binary pages.
        Empty output while running means nothing new yet; this does not wait.
        """
        return await self.call(
            "process_read",
            {
                "process_id": process_id,
                "stdout_offset": stdout_offset,
                "stderr_offset": stderr_offset,
                "max_bytes": max_bytes,
            },
        )

    async def write(
        self,
        process_id: str,
        text: str = "",
        *,
        close_stdin: bool = False,
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
        )

    async def stop(self, process_id: str, *, forget: bool = False) -> dict[str, Any]:
        """Stop the process group, waiting at most two seconds for cleanup.

        forget=True releases completed records/output and invalidates the ID.
        If cleanup is still running, forgotten remains false; inspect status.
        """
        return await self.call("process_stop", {"process_id": process_id, "forget": forget})
