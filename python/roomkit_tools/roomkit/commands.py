"""RoomKit tools that run commands and read their retained output."""

from __future__ import annotations

import json
import os
from collections.abc import Mapping, Sequence
from typing import Any, ClassVar

from ..output import OutputStore
from ..process import DEFAULT_INHERITED_ENV, ProcessSession, ShellTool, _env_options
from ._adapter import _ConfiguredTool, _ToolAdapter


def _roomkit_owner(tools: str) -> str:
    """The trusted owner of the current RoomKit call: its room and acting agent.

    Composed as the RoomKit Rust adapter composes it, compact JSON of
    ``[room_id, channel_id]``, so neither part can be mistaken for the other.
    Model arguments never select it. Outside a RoomKit tool call, refuse.
    """
    from roomkit.core.exceptions import ToolRefusedError
    from roomkit.tools.context import current_tool_call, current_tool_room_id

    room, call = current_tool_room_id(), current_tool_call()
    actor = call.channel_id if call is not None else None
    if not room or not actor:
        raise ToolRefusedError(f"{tools} tools require a conversation and an acting agent")
    return json.dumps([room, actor], ensure_ascii=False, separators=(",", ":"))


class Shell(_ConfiguredTool):
    """The Rust foreground shell, selectable directly in RoomKit's tools list.

    Captures an absolute root without spawning and validates inherit_env/env
    at once. First use resolves the root and validates limits asynchronously.
    A Workspace is accepted as a path only; shell commands do not share its
    mutation lock or filesystem restrictions. With outputs, archives belong to
    the calling room and agent, and a call outside a RoomKit tool call is
    refused. See ShellTool for the command environment, host permissions,
    bounds and cancellation semantics.
    """

    _tool_type = ShellTool

    def __init__(
        self,
        root: str | os.PathLike[str],
        *,
        outputs: OutputStore | None = None,
        inherit_env: Sequence[str] = DEFAULT_INHERITED_ENV,
        env: Mapping[str, str] | None = None,
        max_command_bytes: int | None = None,
        max_output_bytes: int | None = None,
        default_timeout_ms: int | None = None,
        max_timeout_ms: int | None = None,
    ) -> None:
        # Keep the validated copies: later changes to the caller's objects, or
        # an iterator consumed here, cannot change the configured environment.
        environment = _env_options(inherit_env, env)
        self._outputs = outputs
        self._configure(
            root,
            outputs=outputs,
            **environment,
            max_command_bytes=max_command_bytes,
            max_output_bytes=max_output_bytes,
            default_timeout_ms=default_timeout_ms,
            max_timeout_ms=max_timeout_ms,
        )

    async def _call(self, arguments: dict[str, Any]) -> dict[str, Any]:
        owner = None if self._outputs is None else _roomkit_owner("shell archive")
        tool = await self._get_tool()
        return await tool._call(arguments, owner)


class _ProcessAdapter(_ToolAdapter):
    _name: ClassVar[str]

    def __init__(self, session: ProcessSession) -> None:
        if not isinstance(session, ProcessSession):
            raise TypeError("process tools require a shared ProcessSession")
        self._session = session

    @property
    def definition(self) -> dict[str, Any]:
        """Expose this capability only, using the original Rust input schema."""
        definition = ProcessSession.definition(self._name)
        return {
            "name": definition["name"],
            "description": definition["description"],
            "parameters": definition["input_schema"],
        }

    async def _call(self, arguments: dict[str, Any]) -> dict[str, Any]:
        return await self._session._call(self._name, arguments, _roomkit_owner("process"))


class ProcessStart(_ProcessAdapter):
    """Start persistent jobs in a host-selected ProcessSession.

    Pass the same session to selected process tools and close it at shutdown.
    Jobs belong to RoomKit's current room and acting agent, never to a model
    argument: other rooms and agents sharing the session cannot list, read,
    write or stop them. Calls outside a RoomKit tool call are refused.
    """

    _name = "process_start"


class ProcessStatus(_ProcessAdapter):
    """Inspect retained jobs in the supplied ProcessSession without changing them."""

    _name = "process_status"


class ProcessRead(_ProcessAdapter):
    """Read available output in a ProcessSession; never writes or closes stdin."""

    _name = "process_read"


class ProcessWrite(_ProcessAdapter):
    """Write exact text or explicit EOF in a ProcessSession; partial writes are reported."""

    _name = "process_write"


class ProcessStop(_ProcessAdapter):
    """Stop (SIGKILL, no graceful phase) and optionally forget a job in a ProcessSession."""

    _name = "process_stop"


class _OutputAdapter(_ToolAdapter):
    _name: ClassVar[str]

    def __init__(self, store: OutputStore) -> None:
        if not isinstance(store, OutputStore):
            raise TypeError("output tools require an OutputStore")
        self._store = store

    @property
    def definition(self) -> dict[str, Any]:
        definition = OutputStore.definition(self._name)
        return {
            "name": definition["name"],
            "description": definition["description"],
            "parameters": definition["input_schema"],
        }

    async def _call(self, arguments: dict[str, Any]) -> dict[str, Any]:
        return await self._store._call(self._name, arguments, _roomkit_owner("output"))


class OutputRead(_OutputAdapter):
    """Read a disk copy of the current room and agent by ID; register independently."""

    _name = "output_read"


class OutputForget(_OutputAdapter):
    """Explicitly release finished output archives from the selected store."""

    _name = "output_forget"
