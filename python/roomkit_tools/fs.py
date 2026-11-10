"""Async filesystem tools backed by the same Rust implementation as native hosts."""

from __future__ import annotations

import asyncio
import json
import os
from typing import Any, ClassVar, Self

from . import _native
from .workspace import Workspace


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
    """Shared JSON/cancellation boundary for native discovery and comparison tools."""

    _native_type: ClassVar[
        type[_native._ListTool | _native._GlobTool | _native._GrepTool | _native._DiffTool]
    ]

    def __init__(
        self, native: _native._ListTool | _native._GlobTool | _native._GrepTool | _native._DiffTool
    ) -> None:
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

    Files, directories and symlinks can match. Hidden entries are included;
    what the workspace's ``.gitignore`` and ``.ignore`` files exclude, and
    ``.git``, are left out unless ``include_ignored``. Child symlinks are not
    followed."""

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
        include_ignored: bool = False,
        offset: int = 1,
        limit: int | None = None,
    ) -> dict[str, Any]:
        """Find a page of paths; ** crosses directories and * stays in a component."""
        return await self.call(
            {
                "pattern": pattern,
                "path": os.fspath(path),
                "include_ignored": include_ignored,
                "offset": offset,
                "limit": limit,
            }
        )


class GrepTool(_DiscoveryTool):
    """Search UTF-8 files using Rust regex syntax or literal text.

    Matches contain whole source lines and optional context. Hidden files are
    included; what the workspace's ``.gitignore`` and ``.ignore`` files
    exclude, and ``.git``, are not searched unless ``include_ignored``. Child
    symlinks are not followed.
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
        max_line_bytes: int | None = None,
    ) -> Self:
        """Resolve the root; omitted bounds use Rust defaults.

        Defaults: 200 matches, 50 KiB output, 10,000 entries, depth 32,
        8 MiB/file, 32 MiB total input, 20 context lines per side and
        64 KiB lines (longer lines are not searched; see skipped_lines).
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
                "max_line_bytes": max_line_bytes,
            },
        )

    async def grep(
        self,
        pattern: str,
        *,
        path: str | os.PathLike[str] = ".",
        include: str | None = None,
        include_ignored: bool = False,
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
                "include_ignored": include_ignored,
                "literal": literal,
                "ignore_case": ignore_case,
                "context_before": context_before,
                "context_after": context_after,
                "offset": offset,
                "limit": limit,
            }
        )


class _MutationTool:
    """JSON bridge for tools sharing an explicit Rust mutation scope."""

    _native_type: ClassVar[type[_native._WriteTool | _native._EditTool | _native._PatchTool]]

    def __init__(self, native: _native._WriteTool | _native._EditTool | _native._PatchTool) -> None:
        self._native = native

    @classmethod
    async def create(
        cls, root: str | os.PathLike[str], *, max_file_bytes: int | None = None
    ) -> Self:
        """Resolve the root asynchronously; default file bound: 8 MiB.

        Pass the same Workspace object to write, edit and patch to share serialization.
        A plain path creates an independent workspace for this tool.
        """
        return await cls._create(root, {"max_file_bytes": max_file_bytes})

    @classmethod
    async def _create(cls, root: str | os.PathLike[str], options: dict[str, int | None]) -> Self:
        workspace = root if isinstance(root, Workspace) else Workspace(root)
        options = {name: value for name, value in options.items() if value is not None}
        cancel = _native._Cancellation()
        try:
            native = await cls._native_type.create(
                workspace._native, json.dumps(options, allow_nan=False), cancel
            )
        finally:
            cancel.cancel()
        return cls(native)

    @classmethod
    def definition(cls) -> dict[str, Any]:
        """Return fresh input/output schemas derived from the Rust types."""
        return json.loads(cls._native_type.definition_json())

    async def call(self, arguments: dict[str, Any]) -> dict[str, Any]:
        """Execute Rust-validated input and return the data/text envelope.

        Cancellation requests stopping before commit. A commit already started
        may finish even if Python receives CancelledError; read before retrying.
        The Rust worker keeps the shared lock until it finishes.
        """
        cancel = _native._Cancellation()
        try:
            result = await self._native.call(json.dumps(arguments, allow_nan=False), cancel)
        finally:
            cancel.cancel()
        return json.loads(result)


class WriteTool(_MutationTool):
    """Create or explicitly overwrite bounded UTF-8 text beneath a root.

    Missing parents are created automatically. Leaf symlinks and NUL-containing text are
    refused. Exact bytes, including BOM and line endings, are preserved.
    Replacement requires overwrite=True even with expected_revision.
    """

    _native_type = _native._WriteTool

    async def write(
        self,
        path: str | os.PathLike[str],
        content: str,
        *,
        overwrite: bool = False,
        expected_revision: str | None = None,
    ) -> dict[str, Any]:
        """Write complete contents and return flags, byte count and new revision.

        expected_revision is optional; when supplied the destination must
        already exist and match the revision returned by read/write/edit.
        """
        return await self.call(
            {
                "path": os.fspath(path),
                "content": content,
                "overwrite": overwrite,
                "expected_revision": expected_revision,
            }
        )


class EditTool(_MutationTool):
    """Replace exact text while preserving all unmatched bytes.

    old_text must be nonempty and match exactly once unless replace_all=True.
    Empty new_text removes the passage, not the file. Arguments, source and
    result must fit max_file_bytes. Matching uses original whitespace/endings.
    """

    _native_type = _native._EditTool

    async def edit(
        self,
        path: str | os.PathLike[str],
        old_text: str,
        new_text: str,
        *,
        replace_all: bool = False,
        expected_revision: str | None = None,
    ) -> dict[str, Any]:
        """Edit a file and return replacement count, byte counts and new revision."""
        return await self.call(
            {
                "path": os.fspath(path),
                "old_text": old_text,
                "new_text": new_text,
                "replace_all": replace_all,
                "expected_revision": expected_revision,
            }
        )


class DiffTool(_DiscoveryTool):
    """Compare two existing UTF-8 files without modifying them.

    Results contain complete structured hunks and a unified-style text preview.
    BOM and line endings are significant. There is no pagination or clipping:
    oversized/overly complex comparisons fail explicitly. Reads are not a
    snapshot against external edits and do not acquire the mutation lock.
    """

    _native_type = _native._DiffTool

    @classmethod
    async def create(
        cls,
        root: str | os.PathLike[str],
        *,
        max_file_bytes: int | None = None,
        max_lines: int | None = None,
        max_comparison_cells: int | None = None,
        max_output_bytes: int | None = None,
        max_context_lines: int | None = None,
    ) -> Self:
        """Resolve the root; omitted limits retain Rust defaults.

        Defaults: 8 MiB/file, 20,000 lines/file, 1,000,000 comparison cells,
        50 KiB rendered output and 100 context lines/side. Zero context is valid.
        """
        return await cls._create(
            root,
            {
                "max_file_bytes": max_file_bytes,
                "max_lines": max_lines,
                "max_comparison_cells": max_comparison_cells,
                "max_output_bytes": max_output_bytes,
                "max_context_lines": max_context_lines,
            },
        )

    async def diff(
        self,
        old_path: str | os.PathLike[str],
        new_path: str | os.PathLike[str],
        *,
        context_lines: int = 3,
    ) -> dict[str, Any]:
        """Compare files with context capped by the host; no Git history is read."""
        return await self.call(
            {
                "old_path": os.fspath(old_path),
                "new_path": os.fspath(new_path),
                "context_lines": context_lines,
            }
        )


class PatchTool(_MutationTool):
    """Preview or apply structured add/update/delete/move changes.

    This accepts JSON-shaped changes, not a textual Git/Codex patch. Existing
    sources must match complete old_content exactly, including BOM/endings.
    Parents must exist, add/move destinations must be absent, and paths cannot
    overlap. All operations are prepared before any mutation.
    """

    _native_type = _native._PatchTool

    @classmethod
    async def create(
        cls,
        root: str | os.PathLike[str],
        *,
        max_changes: int | None = None,
        max_input_bytes: int | None = None,
        max_file_bytes: int | None = None,
        max_lines: int | None = None,
        max_comparison_cells: int | None = None,
        max_output_bytes: int | None = None,
        max_context_lines: int | None = None,
    ) -> Self:
        """Share a Workspace with write/edit; plain paths create independent scopes.

        Defaults: 32 changes, 16 MiB combined argument bytes, and DiffTool's
        bounds. max_output_bytes caps combined prepared previews; patch context
        is three lines capped by max_context_lines. Zero context is valid.
        """
        return await cls._create(
            root,
            {
                "max_changes": max_changes,
                "max_input_bytes": max_input_bytes,
                "max_file_bytes": max_file_bytes,
                "max_lines": max_lines,
                "max_comparison_cells": max_comparison_cells,
                "max_output_bytes": max_output_bytes,
                "max_context_lines": max_context_lines,
            },
        )

    async def patch(self, changes: list[dict[str, Any]], *, dry_run: bool = True) -> dict[str, Any]:
        """Preview by default; explicit dry_run=False applies a freshly checked batch.

        Inspect completed, applied and failure: there is no multi-file rollback.
        A move records destination creation then source removal as two effects.
        Cancellation can leave committed effects; read files before retrying.
        Preview results are not evidence of application or a reservation.
        """
        return await self.call({"changes": changes, "dry_run": dry_run})
