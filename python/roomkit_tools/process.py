"""Async command execution backed by roomkit-tools-process."""

from __future__ import annotations

import json
import os
from typing import Any

from . import _native


class ShellTool:
    """Run foreground commands using /bin/sh -c, with separate bounded streams.

    Commands inherit the host environment and permissions. The root bounds only
    the initial cwd, not filesystem/network access. No sandbox, mutation lock,
    login shell, PTY, interactive stdin or persistent session is provided.
    Stdin is closed. Unix is implemented; Linux is the validated platform.
    """

    def __init__(self, native: _native._ShellTool) -> None:
        self._native = native

    @classmethod
    async def create(
        cls,
        root: str | os.PathLike[str],
        *,
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
                os.fspath(root), json.dumps(options, allow_nan=False), cancel
            )
        finally:
            cancel.cancel()
        return cls(native)

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
        exit; deliberately detached processes are outside that mechanism.
        Already-performed effects are not rolled back.
        """
        cancel = _native._Cancellation()
        try:
            result = await self._native.call(json.dumps(arguments, allow_nan=False), cancel)
        finally:
            cancel.cancel()
        return json.loads(result)
