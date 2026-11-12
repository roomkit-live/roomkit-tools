"""RoomKit tools that run commands and read their retained output."""

from __future__ import annotations

import json
from typing import Any, ClassVar

from .._native import InvalidInputError
from .._shell import ShellTool
from ..output import OutputStore
from ..process import ProcessSession
from ._adapter import _ToolAdapter, _trusted_caller


def roomkit_owner(room_id: str, channel_id: str) -> str:
    """The owner key of the RoomKit shell, process and output tools in a room.

    Those tools file the jobs and archives of an agent's calls under this key:
    compact JSON of ``[room_id, channel_id]``, the key the RoomKit Rust
    adapter's roomkit_owner returns, so neither part can be mistaken for the
    other. Pass it as owner= to ProcessSession and OutputStore methods to list,
    read, stop or release them. An id that is not Unicode text, such as one
    holding a lone surrogate, raises InvalidInputError: no RoomKit Rust id can
    hold one.
    """
    if not isinstance(room_id, str) or not isinstance(channel_id, str):
        raise TypeError("roomkit_owner requires str room and channel ids")
    owner = json.dumps([room_id, channel_id], ensure_ascii=False, separators=(",", ":"))
    try:
        owner.encode("utf-8")
    except UnicodeEncodeError:
        raise InvalidInputError("room and channel ids must be Unicode text") from None
    return owner


def _roomkit_owner(tools: str) -> str:
    """The trusted owner of the current RoomKit call: its room and acting agent.

    Model arguments never select it. Outside a RoomKit tool call, refuse.
    """
    return roomkit_owner(*_trusted_caller(tools))


class Shell(_ToolAdapter[ShellTool]):
    """A built ShellTool, the foreground shell, in RoomKit's tools list.

    ``Shell(await ShellTool.create(root, outputs=store))``. With an output
    store, archives belong to the calling room and agent, and a call outside a
    RoomKit tool call is refused. See ShellTool for the command environment,
    host permissions, bounds and cancellation semantics.
    """

    _tool_type = ShellTool

    async def _call(self, arguments: dict[str, Any]) -> dict[str, Any]:
        # Archives need the caller; without them, a call outside RoomKit runs
        # standalone, and one inside files its kept jobs under its caller,
        # whom their end is told to.
        if self._tool._outputs is not None:
            owner = _roomkit_owner("shell archive")
        else:
            from roomkit.core.exceptions import ToolRefusedError

            try:
                owner = _roomkit_owner("shell")
            except ToolRefusedError:
                owner = None
        return await self._tool._call(arguments, owner)


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
        owner = _roomkit_owner("process")
        return await self._session.call(self._name, arguments, owner=owner)


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
        owner = _roomkit_owner("output")
        return await self._store.call(self._name, arguments, owner=owner)


class OutputRead(_OutputAdapter):
    """Read a disk copy of the current room and agent by ID; register independently."""

    _name = "output_read"


class OutputForget(_OutputAdapter):
    """Explicitly release finished output archives from the selected store."""

    _name = "output_forget"
