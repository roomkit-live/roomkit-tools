"""Foreground commands and their host environment, backed by roomkit-tools-process."""

from __future__ import annotations

import json
import os
from collections.abc import Mapping, Sequence
from typing import Any

from . import _native
from .output import OutputStore

DEFAULT_INHERITED_ENV: tuple[str, ...] = tuple(_native.DEFAULT_INHERITED_ENV)
"""Host variables commands receive by default, when the host has them set."""


def _env_options(inherit_env: Sequence[str], env: Mapping[str, str] | None) -> dict[str, Any]:
    """Check the environment options' types here and their content in Rust."""
    if isinstance(inherit_env, str | bytes):
        raise TypeError("inherit_env requires a sequence of variable names, not one string")
    names = list(inherit_env)
    if not all(isinstance(name, str) for name in names):
        raise TypeError("inherit_env requires str variable names")
    if env is not None and not isinstance(env, Mapping):
        raise TypeError("env requires a mapping of variable names to values")
    values = dict(env or {})
    if not all(isinstance(name, str) and isinstance(value, str) for name, value in values.items()):
        raise TypeError("env requires str variable names and values")
    options = {"inherit_env": names, "env": values}
    _native._check_command_env(json.dumps(options))
    return options


class ShellTool:
    """Run foreground commands using /bin/sh -c, with separate bounded streams.

    Commands run with the host's permissions. They receive only the host
    variables named in inherit_env (default DEFAULT_INHERITED_ENV) that are set,
    plus explicit env values. The root bounds only the initial cwd, not
    filesystem/network access. No sandbox, mutation lock, login shell, PTY,
    interactive stdin or persistent session is provided.
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
        inherit_env: Sequence[str] = DEFAULT_INHERITED_ENV,
        env: Mapping[str, str] | None = None,
        max_command_bytes: int | None = None,
        max_output_bytes: int | None = None,
        default_timeout_ms: int | None = None,
        max_timeout_ms: int | None = None,
        keep_background_jobs: bool | None = None,
    ) -> ShellTool:
        """Resolve an existing root; omitted limits use Rust defaults.

        Commands receive the host variables named in inherit_env that are set
        when they start, then the env values, which replace inherited ones.
        Names must be nonempty without '=' or NUL; values must not contain NUL
        (InvalidInputError). Defaults: 16 KiB command, 32 KiB retained raw
        bytes per stream, 10 s default runtime and 120 s maximum. Bounds must be
        positive; the default timeout must not exceed the maximum, which cannot
        exceed 24 hours. keep_background_jobs leaves a command's background jobs
        running once its shell exits on its own, instead of stopping its whole
        process group; a job still holding the command's pipes then writes to a
        drainer that discards its output and outlives the host. A timeout or
        cancellation still stops every member (default False).
        """
        if outputs is not None and not isinstance(outputs, OutputStore):
            raise TypeError("outputs requires an OutputStore")
        environment = _env_options(inherit_env, env)
        options = {
            name: value
            for name, value in {
                "max_command_bytes": max_command_bytes,
                "max_output_bytes": max_output_bytes,
                "default_timeout_ms": default_timeout_ms,
                "max_timeout_ms": max_timeout_ms,
                "keep_background_jobs": keep_background_jobs,
            }.items()
            if value is not None
        } | environment
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
        """Run with a per-call cwd and a runtime within the host maximum.

        Nonzero exits and timeouts are results, not tool exceptions. Streams
        retain their most recent bytes, with omission metadata and lossy UTF-8
        text; with an output store, only a stream cut short keeps an archive.
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
        exit, unless keep_background_jobs keeps a normal exit's background jobs;
        deliberately detached processes and members under other credentials are
        outside that mechanism, and the result is still returned.
        Already-performed effects are not rolled back.
        """
        return await self._call(arguments, None)

    async def _call(self, arguments: dict[str, Any], owner: str | None) -> dict[str, Any]:
        """Run for a trusted archive owner key; None is the standalone owner."""
        cancel = _native._Cancellation()
        try:
            arguments_json = json.dumps(arguments, allow_nan=False)
            result = await self._native.call(arguments_json, cancel, owner)
        finally:
            cancel.cancel()
        return json.loads(result)
