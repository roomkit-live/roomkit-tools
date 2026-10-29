"""Independent Rust/Python syntax outlines; all parsing takes place in Rust."""

from __future__ import annotations

import json
import os
from typing import Any, Literal, Self

from . import _native


class CodeOutlineTool:
    """Outline one UTF-8 source file without executing it or resolving types.

    Calls return definitions with signatures and inclusive line ranges. Offsets
    count symbols, not lines. Carry the raw-file revision when paging; syntax
    errors and clipped signatures are explicit. Each call reparses the file.
    Clones of the native tool share two workers; cancelling the caller signals
    cooperative parser/traversal cancellation. Limits are not an OS sandbox.
    """

    def __init__(self, native: _native._CodeOutlineTool) -> None:
        self._native = native

    @classmethod
    async def create(
        cls,
        root: str | os.PathLike[str],
        *,
        max_file_bytes: int | None = None,
        max_output_bytes: int | None = None,
        max_symbols: int | None = None,
        max_scan_symbols: int | None = None,
        max_scan_nodes: int | None = None,
        max_depth: int | None = None,
        max_signature_bytes: int | None = None,
        default_timeout_ms: int | None = None,
        max_timeout_ms: int | None = None,
    ) -> Self:
        """Resolve the root asynchronously; omitted bounds use Rust defaults."""
        options = {
            key: value
            for key, value in {
                "max_file_bytes": max_file_bytes,
                "max_output_bytes": max_output_bytes,
                "max_symbols": max_symbols,
                "max_scan_symbols": max_scan_symbols,
                "max_scan_nodes": max_scan_nodes,
                "max_depth": max_depth,
                "max_signature_bytes": max_signature_bytes,
                "default_timeout_ms": default_timeout_ms,
                "max_timeout_ms": max_timeout_ms,
            }.items()
            if value is not None
        }
        token = _native._Cancellation()
        try:
            native = await _native._CodeOutlineTool.create(
                os.fspath(root), json.dumps(options, allow_nan=False), token
            )
        finally:
            token.cancel()
        return cls(native)

    @staticmethod
    def definition() -> dict[str, Any]:
        """Fresh schemas derived from the Rust request and response types."""
        return json.loads(_native._CodeOutlineTool.definition_json())

    async def call(self, arguments: dict[str, Any]) -> dict[str, Any]:
        """Execute Rust-validated input and preserve typed errors and cancellation."""
        token = _native._Cancellation()
        try:
            return json.loads(
                await self._native.call(json.dumps(arguments, allow_nan=False), token)
            )
        finally:
            token.cancel()

    async def outline(
        self,
        path: str | os.PathLike[str],
        *,
        language: Literal["rust", "python"] | None = None,
        offset: int = 1,
        limit: int | None = None,
        expected_revision: str | None = None,
        timeout_ms: int | None = None,
    ) -> dict[str, Any]:
        """Return a page of definitions, preserving raw-file revision and ranges."""
        return await self.call(
            {
                "path": os.fspath(path),
                "language": language,
                "offset": offset,
                "limit": limit,
                "expected_revision": expected_revision,
                "timeout_ms": timeout_ms,
            }
        )
