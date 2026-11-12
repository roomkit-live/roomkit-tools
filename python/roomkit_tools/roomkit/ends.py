"""A background job's end, told to the agent that started it."""

from __future__ import annotations

import asyncio
import json
import logging
from collections.abc import AsyncIterator
from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from roomkit import RoomKit

logger = logging.getLogger("roomkit_tools")

_COMMAND_CHARS = 200
"""The longest command a notice quotes, in characters."""


async def notify_job_ends(kit: RoomKit, ends: AsyncIterator[dict[str, Any]]) -> None:
    """Tell each RoomKit agent when a background job it started ends.

    *ends* is a ProcessSession's or a ShellTool's ``ends()``, taken before the
    agents start their jobs; run one task per session,
    ``asyncio.create_task(notify_job_ends(kit, ends))``. Each end of a job the
    process or shell adapters started reaches its agent as an instruction
    (``deliver()`` with ``instruction=True``, addressed to it), which gives the
    agent a turn, or is read by the turn it is running. A delivery lasts the
    turn it gives, so each runs in a task of its own: one agent's turn never
    holds back another's notice. A job stopped, by the agent, the host or a
    close, is not told: whoever stopped it knows. An end whose owner is not a
    RoomKit room and agent is skipped; one the kit refuses is logged, unless
    it closed. Returns once *ends* stopped and every delivery begun has ended.
    """
    delivering: set[asyncio.Task[None]] = set()
    async for end in ends:
        caller, notice = _caller(end.get("owner")), _notice(end)
        if caller is None or notice is None:
            continue
        task = asyncio.create_task(_tell(kit, *caller, notice))
        delivering.add(task)
        task.add_done_callback(delivering.discard)
    if delivering:
        await asyncio.gather(*delivering)


async def _tell(kit: RoomKit, room_id: str, channel_id: str, notice: str) -> None:
    """Deliver *notice* to *channel_id* in *room_id*, as the application's instruction."""
    import roomkit

    try:
        await kit.deliver(room_id, notice, addressed_to=[channel_id], instruction=True)
    except roomkit.RoomKitError as error:
        closed = getattr(roomkit, "KitClosedError", None)
        if closed is None or not isinstance(error, closed):
            logger.warning(
                "the end of a background job was not told in room %s: %s", room_id, error
            )


def _caller(owner: str | None) -> tuple[str, str] | None:
    """The room and agent an owner key names: roomkit_owner's."""
    if owner is None:
        return None
    try:
        parts = json.loads(owner)
    except ValueError:
        return None
    if (
        not isinstance(parts, list)
        or len(parts) != 2
        or not all(isinstance(part, str) and part for part in parts)
    ):
        return None
    return parts[0], parts[1]


def _notice(end: dict[str, Any]) -> str | None:
    """What the agent reads of *end*; None for a stop, which it is not told."""
    if end["end"] == "stopped":
        return None
    command, after = _quoted(end["command"]), _duration(end["elapsed_ms"])
    if end["tool"] == "shell":
        return (
            f"The background jobs of the shell command {command} ended after {after}. "
            "Their exit status is unknown: check what they left (logs, files, ports)."
        )
    process = f"Background process {end['process_id'] or '?'} ({command})"
    read = "Read its output with process_read."
    if end["end"] == "timed_out":
        return f"{process} reached its time limit after {after} and was stopped. {read}"
    if end["end"] == "failed":
        return f"{process} failed after {after}; process_status says why."
    if end["exit_code"] is not None:
        return f"{process} exited with code {end['exit_code']} after {after}. {read}"
    if end["signal"] is not None:
        return f"{process} was killed by signal {end['signal']} after {after}. {read}"
    return f"{process} ended after {after}. {read}"


def _quoted(command: str) -> str:
    """*command* on one line, cut to _COMMAND_CHARS, in backquotes."""
    line = " ".join(command.split())
    shown = line[:_COMMAND_CHARS] + ("…" if len(line) > _COMMAND_CHARS else "")
    return f"`{shown}`"


def _duration(ms: int) -> str:
    """*ms* as a person says it: 850ms, 42s, 2m13s, 1h05m."""
    seconds = ms // 1000
    if seconds == 0:
        return f"{ms}ms"
    if seconds < 60:
        return f"{seconds}s"
    if seconds < 3600:
        return f"{seconds // 60}m{seconds % 60:02}s"
    return f"{seconds // 3600}h{seconds % 3600 // 60:02}m"
