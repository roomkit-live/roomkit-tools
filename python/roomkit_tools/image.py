"""Local image preparation in Rust; no Python imaging or model dependency."""

from __future__ import annotations

import json
import os
from typing import Any, Self

from . import _native


class ViewImageTool:
    """Prepare static PNG/JPEG/WebP as a bounded 8-bit PNG vision payload.

    Sources are unchanged. Results report orientation, resizing, dimensions and
    the original file revision. The image field is multimodal data, not prose.
    Clones of the native tool share two workers; cancellation cannot preempt a
    decoder/resize stage but the slot stays owned until that work stops.
    """

    def __init__(self, native: _native._ViewImageTool) -> None:
        self._native = native

    @classmethod
    async def create(
        cls,
        root: str | os.PathLike[str],
        *,
        max_file_bytes: int | None = None,
        max_dimension: int | None = None,
        max_pixels: int | None = None,
        max_decode_bytes: int | None = None,
        max_edge: int | None = None,
        max_image_bytes: int | None = None,
        default_timeout_ms: int | None = None,
        max_timeout_ms: int | None = None,
    ) -> Self:
        """Resolve the root; omitted host bounds use Rust defaults."""
        options = {
            key: value
            for key, value in {
                "max_file_bytes": max_file_bytes,
                "max_dimension": max_dimension,
                "max_pixels": max_pixels,
                "max_decode_bytes": max_decode_bytes,
                "max_edge": max_edge,
                "max_image_bytes": max_image_bytes,
                "default_timeout_ms": default_timeout_ms,
                "max_timeout_ms": max_timeout_ms,
            }.items()
            if value is not None
        }
        token = _native._Cancellation()
        try:
            native = await _native._ViewImageTool.create(
                os.fspath(root), json.dumps(options, allow_nan=False), token
            )
        finally:
            token.cancel()
        return cls(native)

    @staticmethod
    def definition() -> dict[str, Any]:
        """Input/output schemas derived from the actual Rust types."""
        return json.loads(_native._ViewImageTool.definition_json())

    async def call(self, arguments: dict[str, Any]) -> dict[str, Any]:
        """Return metadata and complete base64 pixels; never a truncated image."""
        token = _native._Cancellation()
        try:
            return json.loads(
                await self._native.call(json.dumps(arguments, allow_nan=False), token)
            )
        finally:
            token.cancel()

    async def view(
        self, path: str | os.PathLike[str], *, timeout_ms: int | None = None
    ) -> dict[str, Any]:
        """Prepare a regular local image beneath the host's root."""
        return await self.call({"path": os.fspath(path), "timeout_ms": timeout_ms})
