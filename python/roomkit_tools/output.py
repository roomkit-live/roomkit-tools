"""Optional owner-scoped disk copies of command output, implemented in Rust."""

from __future__ import annotations

import json
import os
import tempfile
from types import TracebackType
from typing import Any, Self

from . import _native


class OutputStore:
    """Ephemeral disk copies shared by selected shell and process tools.

    Pass it as ``outputs=store`` to Shell/ShellTool or ProcessSession; select
    OutputRead and optionally OutputForget for the agent. Every archive belongs
    to the owner of the command that produced it: the RoomKit tools give each
    room and acting agent its own owner, roomkit_owner(room_id, channel_id), so
    one store can serve several conversations. The host methods take owner=;
    None, the default, is one standalone owner, apart from every named owner,
    "default" included. No arbitrary paths are accepted by those tools. The
    host controls the parent directory and quotas.

    Archives survive process-record deletion and session close. Explicit forget
    or store close releases them; nothing is promised across host restarts.
    Use an outer async context so processes close before their output store.
    """

    def __init__(
        self,
        parent: str | os.PathLike[str] | None = None,
        *,
        max_archives: int | None = None,
        max_archives_per_owner: int | None = None,
        max_stream_bytes: int | None = None,
        max_total_bytes: int | None = None,
        max_read_bytes: int | None = None,
        write_timeout_ms: int | None = None,
    ) -> None:
        """Configure lazy Rust storage, defaulting to the system temporary parent.

        Defaults: 64 retained streams, 16 per owner (the standalone owner, or
        one room and agent), 8 MiB per stream, 64 MiB total, 16 KiB/read, 1 s
        writer wait.
        Stored and queued reservations count toward quota; failed reservations
        remain charged until forget/close. All bounds must be positive.
        A stream over quota retains a prefix and reports incomplete metadata.
        """
        path = tempfile.gettempdir() if parent is None else os.fspath(parent)
        if not path:
            raise _native.InvalidInputError("output parent must not be empty")
        options = {
            name: value
            for name, value in {
                "max_archives": max_archives,
                "max_archives_per_owner": max_archives_per_owner,
                "max_stream_bytes": max_stream_bytes,
                "max_total_bytes": max_total_bytes,
                "max_read_bytes": max_read_bytes,
                "write_timeout_ms": write_timeout_ms,
            }.items()
            if value is not None
        }
        self._native = _native._OutputStore(
            os.path.abspath(path), json.dumps(options, allow_nan=False)
        )

    @property
    def closed(self) -> bool:
        """Whether shutdown was requested; it may still have filesystem work pending."""
        return self._native.is_closed()

    async def __aenter__(self) -> Self:
        await self.call("initialize", {})
        return self

    async def __aexit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        if not await self.close():
            raise _native.ToolError("Output cleanup is pending; await store.close() again")

    @staticmethod
    def definition(name: str) -> dict[str, Any]:
        """Return the Rust-derived output_read or output_forget definition."""
        return json.loads(_native._OutputStore.definition_json(name))

    async def call(self, name: str, arguments: dict[str, Any], *, owner: str | None = None) -> Any:
        """Execute Rust-validated input; preserve structured metadata and text.

        owner names whose archives the operation reaches, a trusted host
        string, never a model argument; None is the standalone owner.
        """
        token = _native._Cancellation()
        try:
            arguments_json = json.dumps(arguments, allow_nan=False)
            return json.loads(await self._native.call(name, arguments_json, token, owner))
        finally:
            token.cancel()

    async def read(
        self,
        archive_id: str,
        *,
        offset: int = 0,
        max_bytes: int | None = None,
        owner: str | None = None,
    ) -> dict[str, Any]:
        """Read confirmed bytes without consuming them. Check archive.complete/error.

        The available prefix can grow while a process or writer is active.
        has_more=False only describes this snapshot. bytes preserves binary
        output; text is lossy when a page splits UTF-8. No command is rerun.
        """
        return await self.call(
            "output_read",
            {"archive_id": archive_id, "offset": offset, "max_bytes": max_bytes},
            owner=owner,
        )

    async def forget(self, archive_id: str, *, owner: str | None = None) -> dict[str, Any]:
        """Release a finished archive and its quota. Active capture is refused."""
        return await self.call("output_forget", {"archive_id": archive_id}, owner=owner)

    async def list(self, *, owner: str | None = None) -> list[dict[str, Any]]:
        """Host API to recover the owner's retained IDs, in creation order.

        Includes calls whose response was cancelled. Archives of commands run
        through the RoomKit tools belong to their room and agent: pass
        owner=roomkit_owner(room_id, channel_id).
        """
        return await self.call("list", {}, owner=owner)

    async def close(self) -> bool:
        """Permanently close and wait up to two seconds for writer cleanup.

        Once issued, shutdown continues if this awaiter is cancelled. False
        means filesystem work is still pending; call close again. Close does
        not terminate commands: their previews continue with copy errors.
        Explicit close is preferred to last-reference cleanup at interpreter exit.
        """
        return await self._native.close()
