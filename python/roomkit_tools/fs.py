"""Async filesystem tools backed by the same Rust implementation as native hosts."""

from __future__ import annotations

import asyncio
import json
import os
from typing import Any, ClassVar, Self

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


class _DiscoveryTool:
    """Shared JSON/cancellation boundary for the three native discovery tools."""

    _native_type: ClassVar[type[_native._ListTool | _native._GlobTool | _native._GrepTool]]

    def __init__(self, native: _native._ListTool | _native._GlobTool | _native._GrepTool) -> None:
        self._native = native

    @classmethod
    async def _create(cls, root: str | os.PathLike[str], options: dict[str, int | None]) -> Self:
        options = {name: value for name, value in options.items() if value is not None}
        cancel = _native._Cancellation()
        try:
            native = await cls._native_type.create(
                os.fspath(root), json.dumps(options, allow_nan=False), cancel
            )
        finally:
            cancel.cancel()
        return cls(native)

    @classmethod
    def definition(cls) -> dict[str, Any]:
        """Return fresh input/output schemas derived from the Rust types."""
        return json.loads(cls._native_type.definition_json())

    async def call(self, arguments: dict[str, Any]) -> dict[str, Any]:
        """Execute Rust-validated input and return its data/text envelope.

        Each call has its own cancellation token. Cancelling the Python task
        cancels its native future; already-issued I/O may still finish.
        """
        cancel = _native._Cancellation()
        try:
            result = await self._native.call(json.dumps(arguments, allow_nan=False), cancel)
        finally:
            cancel.cancel()
        return json.loads(result)


class ListTool(_DiscoveryTool):
    """List sorted immediate directory entries, including hidden entries.

    Child symlinks are listed without following them. Each call rescans the
    directory; pagination is not a snapshot across external mutations."""

    _native_type = _native._ListTool

    @classmethod
    async def create(
        cls,
        root: str | os.PathLike[str],
        *,
        max_entries: int | None = None,
        max_output_bytes: int | None = None,
        max_scan_entries: int | None = None,
    ) -> Self:
        """Resolve the root; defaults: 200 entries, 50 KiB output, 10,000 scanned entries."""
        return await cls._create(
            root,
            {
                "max_entries": max_entries,
                "max_output_bytes": max_output_bytes,
                "max_scan_entries": max_scan_entries,
            },
        )

    async def list(
        self, path: str | os.PathLike[str] = ".", *, offset: int = 1, limit: int | None = None
    ) -> dict[str, Any]:
        """List a page from a one-based entry offset; paths are workspace-relative."""
        return await self.call({"path": os.fspath(path), "offset": offset, "limit": limit})


class GlobTool(_DiscoveryTool):
    """Find paths using Rust glob syntax beneath a configured root.

    Files, directories and symlinks can match. Hidden entries are included,
    ignore files are not applied and child symlinks are not followed."""

    _native_type = _native._GlobTool

    @classmethod
    async def create(
        cls,
        root: str | os.PathLike[str],
        *,
        max_matches: int | None = None,
        max_output_bytes: int | None = None,
        max_scan_entries: int | None = None,
        max_depth: int | None = None,
    ) -> Self:
        """Resolve the root; defaults: 200 matches, 50 KiB output, 10,000 entries, depth 32."""
        return await cls._create(
            root,
            {
                "max_matches": max_matches,
                "max_output_bytes": max_output_bytes,
                "max_scan_entries": max_scan_entries,
                "max_depth": max_depth,
            },
        )

    async def glob(
        self,
        pattern: str,
        *,
        path: str | os.PathLike[str] = ".",
        offset: int = 1,
        limit: int | None = None,
    ) -> dict[str, Any]:
        """Find a page of paths; ** crosses directories and * stays in a component."""
        return await self.call(
            {"pattern": pattern, "path": os.fspath(path), "offset": offset, "limit": limit}
        )


class GrepTool(_DiscoveryTool):
    """Search UTF-8 files using Rust regex syntax or literal text.

    Matches contain whole source lines and optional context. Hidden files are
    included, ignore files are not applied and child symlinks are not followed.
    Binary/non-UTF-8 files are skipped and counted; other failures are raised."""

    _native_type = _native._GrepTool

    @classmethod
    async def create(
        cls,
        root: str | os.PathLike[str],
        *,
        max_matches: int | None = None,
        max_output_bytes: int | None = None,
        max_scan_entries: int | None = None,
        max_depth: int | None = None,
        max_file_bytes: int | None = None,
        max_total_bytes: int | None = None,
        max_context_lines: int | None = None,
    ) -> Self:
        """Resolve the root; omitted bounds use Rust defaults.

        Defaults: 200 matches, 50 KiB output, 10,000 entries, depth 32,
        8 MiB/file, 32 MiB total input and 20 context lines per side.
        max_context_lines=0 disables context; other bounds must be positive."""
        return await cls._create(
            root,
            {
                "max_matches": max_matches,
                "max_output_bytes": max_output_bytes,
                "max_scan_entries": max_scan_entries,
                "max_depth": max_depth,
                "max_file_bytes": max_file_bytes,
                "max_total_bytes": max_total_bytes,
                "max_context_lines": max_context_lines,
            },
        )

    async def grep(
        self,
        pattern: str,
        *,
        path: str | os.PathLike[str] = ".",
        include: str | None = None,
        literal: bool = False,
        ignore_case: bool = False,
        context_before: int = 0,
        context_after: int = 0,
        offset: int = 1,
        limit: int | None = None,
    ) -> dict[str, Any]:
        """Return a page of matching lines; context does not advance the match offset."""
        return await self.call(
            {
                "pattern": pattern,
                "path": os.fspath(path),
                "include": include,
                "literal": literal,
                "ignore_case": ignore_case,
                "context_before": context_before,
                "context_after": context_after,
                "offset": offset,
                "limit": limit,
            }
        )
