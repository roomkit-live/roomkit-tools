"""Selectable OCR facade; recognition engines are separately installed backends."""

from __future__ import annotations

import asyncio
import json
import os
from typing import Any, Protocol, Self

from . import _native


class BoundOcrBackend(Protocol):
    """Host-trusted implementation preserving the shared OCR contract and limits."""

    async def call(self, arguments: dict[str, Any]) -> dict[str, Any]:
        """Execute OCR, propagating typed roomkit-tools errors and cancellation."""
        ...


class OcrBackend(Protocol):
    """Reusable model owner; bind separate roots without loading duplicate models."""

    async def bind(self, root: str, limits: dict[str, int]) -> BoundOcrBackend:
        """Resolve a root and preserve host limits in the bound implementation."""
        ...


class OcrTool:
    """Standalone OCR using an explicit host backend; no default model downloads."""

    def __init__(self, implementation: BoundOcrBackend) -> None:
        self._implementation = implementation

    @classmethod
    async def create(
        cls, root: str | os.PathLike[str], *, backend: OcrBackend, **limits: int | None
    ) -> Self:
        """Bind a workspace to a backend; omitted bounds use Rust defaults."""
        path = os.fspath(root)
        if not path:
            raise _native.InvalidInputError("root must not be empty")
        implementation = await backend.bind(
            await asyncio.to_thread(os.path.abspath, path),
            {key: value for key, value in limits.items() if value is not None},
        )
        return cls(implementation)

    @staticmethod
    def definition() -> dict[str, Any]:
        """Input/output schemas derived from roomkit-tools-ocr's actual Rust types."""
        return json.loads(_native.ocr_definition_json())

    async def call(self, arguments: dict[str, Any]) -> dict[str, Any]:
        """Return bounded text/blocks; scores do not guarantee transcription accuracy."""
        return await self._implementation.call(arguments)

    async def ocr(
        self,
        path: str | os.PathLike[str],
        *,
        rotation_degrees: int = 0,
        expected_revision: str | None = None,
        timeout_ms: int | None = None,
    ) -> dict[str, Any]:
        """Transcribe one static image; rotation is clockwise after EXIF orientation."""
        return await self.call(
            {
                "path": os.fspath(path),
                "rotation_degrees": rotation_degrees,
                "expected_revision": expected_revision,
                "timeout_ms": timeout_ms,
            }
        )
