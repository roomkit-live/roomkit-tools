"""Async filesystem tools backed by the same Rust implementation as native hosts."""

from __future__ import annotations

import asyncio
import json
import os
from typing import Any

from . import _native


class ReadTool:
    """Read bounded UTF-8 pages beneath a host-selected root.

    Construct with ``await ReadTool.create(root)``. Calls return the Rust
    ``{"data": ..., "text": ...}`` envelope as a Python dictionary. Cancelling
    an asyncio task cancels its Rust token and future. An already-issued Tokio
    filesystem operation may still finish; this tool performs no writes.
    """

    def __init__(self, native: _native._ReadTool) -> None:
        self._native = native

    @classmethod
    async def create(
        cls,
        root: str | os.PathLike[str],
        *,
        max_file_bytes: int | None = None,
        max_output_bytes: int | None = None,
        max_lines: int | None = None,
    ) -> ReadTool:
        """Resolve the root asynchronously; omitted bounds use Rust defaults.

        Defaults are 8 MiB ingested per file, 50 KiB of rendered output and
        2,000 lines per page. All configured bounds must be positive.
        """
        options = {
            name: value
            for name, value in {
                "max_file_bytes": max_file_bytes,
                "max_output_bytes": max_output_bytes,
                "max_lines": max_lines,
            }.items()
            if value is not None
        }
        cancel = _native._Cancellation()
        try:
            native = await _native._ReadTool.create(
                os.fspath(root), json.dumps(options, allow_nan=False), cancel
            )
        finally:
            cancel.cancel()
        return cls(native)

    @staticmethod
    def definition() -> dict[str, Any]:
        """Return a fresh copy of the definition and both Rust-derived schemas."""
        return json.loads(_native._ReadTool.definition_json())

    async def read(
        self, path: str | os.PathLike[str], *, offset: int = 1, limit: int | None = None
    ) -> dict[str, Any]:
        """Read from a one-based line offset, returning structured data and text."""
        return await self.call({"path": os.fspath(path), "offset": offset, "limit": limit})

    async def call(self, arguments: dict[str, Any]) -> dict[str, Any]:
        """Execute schema-shaped arguments, validated by the actual Rust input type.

        Unknown fields, booleans in integer fields, zero offsets and zero limits
        are rejected by Rust. JSON encoding errors remain Python TypeError or
        ValueError. Cancellation propagates as asyncio.CancelledError.
        """
        cancel = _native._Cancellation()
        try:
            result = await self._native.read(json.dumps(arguments, allow_nan=False), cancel)
        except asyncio.CancelledError:
            cancel.cancel()
            raise
        finally:
            cancel.cancel()
        return json.loads(result)
