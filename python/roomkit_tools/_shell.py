"""Foreground commands and their host environment, backed by roomkit-tools-process."""

from __future__ import annotations

import json
import os
from collections.abc import AsyncIterator, Mapping, Sequence
from typing import TYPE_CHECKING, Any

from . import _native
from ._ends import _JobEnds
from .output import OutputStore

if TYPE_CHECKING:
    from .process import ProcessSession

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
    With a ProcessSession, a command runs on in the background as one of its
    processes when started with run_in_background, or when background_on_timeout
    moves it there at its timeout.
    """

    def __init__(
        self,
        native: _native._ShellTool,
        outputs: OutputStore | None = None,
        processes: ProcessSession | None = None,
    ) -> None:
        self._native = native
        self._outputs = outputs
        # Kept alive with the tool: dropping the last reference closes it.
        self._processes = processes

    @classmethod
    async def create(
        cls,
        root: str | os.PathLike[str],
        *,
        outputs: OutputStore | None = None,
        processes: ProcessSession | None = None,
        inherit_env: Sequence[str] = DEFAULT_INHERITED_ENV,
        env: Mapping[str, str] | None = None,
        max_command_bytes: int | None = None,
        max_output_bytes: int | None = None,
        default_timeout_ms: int | None = None,
        max_timeout_ms: int | None = None,
        keep_background_jobs: bool | None = None,
        background_on_timeout: bool | None = None,
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
        cancellation still stops every member (default False). The tool
        follows the jobs it keeps: ends() tells when they ended, close() stops
        them and detach() lets them go.

        processes runs background commands as processes of that ProcessSession:
        those started with run_in_background, and those background_on_timeout
        moves there. They take its identifiers, quotas, lifetimes, output
        retention and ends(), under the caller's owner, so its process tools
        read and stop them; they keep this tool's environment, and stdin stays
        closed. Closing this tool leaves them to the session.
        background_on_timeout moves a command still running at its timeout to
        the background instead of stopping it: it runs on until the session's
        maximum lifetime counted from its start, and the result names it in
        process_id. A move the session refuses, its quota reached, stops the
        command as without it. It needs processes (InvalidInputError
        otherwise; default False).
        """
        from .process import ProcessSession

        if outputs is not None and not isinstance(outputs, OutputStore):
            raise TypeError("outputs requires an OutputStore")
        if processes is not None and not isinstance(processes, ProcessSession):
            raise TypeError("processes requires a ProcessSession")
        environment = _env_options(inherit_env, env)
        options = {
            name: value
            for name, value in {
                "max_command_bytes": max_command_bytes,
                "max_output_bytes": max_output_bytes,
                "default_timeout_ms": default_timeout_ms,
                "max_timeout_ms": max_timeout_ms,
                "keep_background_jobs": keep_background_jobs,
                "background_on_timeout": background_on_timeout,
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
                None if processes is None else processes._native,
            )
        finally:
            cancel.cancel()
        return cls(native, outputs, processes)

    @staticmethod
    def definition() -> dict[str, Any]:
        """Return fresh input/output schemas derived from the Rust types."""
        return json.loads(_native._ShellTool.definition_json())

    def ends(self) -> AsyncIterator[dict[str, Any]]:
        """Follow the background jobs that commands leave running from now on.

        With keep_background_jobs, ``async for end in tool.ends()`` gives one
        dict once every job a command left has ended, whatever its owner:
        owner, tool ("shell"), process_id (None), command, cwd, end, exit_code
        and signal (None: the jobs are not the host's children) and
        elapsed_ms. A job that leaves the command's process group, such as a
        daemon, is not followed. Call it before running the commands to
        follow; the iteration stops once the tool closes. owner is the host's
        key: filter by it before telling another principal.
        """
        return _JobEnds(self._native.ends())

    async def close(self) -> bool:
        """Permanently close the tool: stop the background jobs it follows.

        Their ends are told, then ends() stops; later commands raise
        AccessRefusedError. True means every job stopped within two seconds.
        Dropping the tool instead leaves its jobs running, as before the tool
        followed them.
        """
        return await self._native.close()

    async def detach(self) -> bool:
        """Permanently close the tool but leave the jobs it follows running, untold.

        For a host that exits while a job must outlive it. True means every
        job was let go within two seconds.
        """
        return await self._native.detach()

    async def shell(
        self,
        command: str,
        *,
        cwd: str | os.PathLike[str] | None = None,
        timeout_ms: int | None = None,
        run_in_background: bool = False,
    ) -> dict[str, Any]:
        """Run with a per-call cwd and a runtime within the host maximum.

        Nonzero exits and timeouts are results, not tool exceptions. Streams
        retain their most recent bytes, with omission metadata and lossy UTF-8
        text; with an output store, only a stream cut short keeps an archive.
        A timeout returns partial output; output_complete says whether pipes
        reached EOF. Runtime starts after spawn, excluding path resolution;
        termination/reaping can extend wall-clock duration beyond the timeout.
        run_in_background starts the command as a process of the tool's
        ProcessSession and returns its process_id at once; timeout_ms is then
        its lifetime, within the session's limits. Without a session it raises
        InvalidInputError. A result with a process_id runs on in the
        background, started there or moved there at its timeout: exit_code and
        signal are None, and the session's process tools read the rest.
        """
        return await self.call(
            {
                "command": command,
                "cwd": None if cwd is None else os.fspath(cwd),
                "timeout_ms": timeout_ms,
                "run_in_background": run_in_background,
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
