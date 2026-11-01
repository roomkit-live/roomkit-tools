"""RoomKit tools that view or transcribe local images."""

from __future__ import annotations

import json
import os
from typing import Any

from .._native import (
    AccessRefusedError,
    InvalidInputError,
    LimitExceededError,
    UnsupportedResourceError,
)
from ..image import ViewImageTool
from ..ocr import OcrBackend, OcrTool
from ._adapter import _ConfiguredTool, _ToolAdapter


class ViewImage(_ConfiguredTool):
    """Select local image pixels as RoomKit multimodal tool output.

    Register with a vision-capable model. The handler returns typed text/image
    parts, never base64 in ordinary model text. Limits are ViewImageTool.create's
    host options. Static PNG/JPEG/WebP only; no OCR or remote URL fetching.
    """

    _tool_type = ViewImageTool

    def __init__(
        self,
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
    ) -> None:
        self._configure(
            root,
            max_file_bytes=max_file_bytes,
            max_dimension=max_dimension,
            max_pixels=max_pixels,
            max_decode_bytes=max_decode_bytes,
            max_edge=max_edge,
            max_image_bytes=max_image_bytes,
            default_timeout_ms=default_timeout_ms,
            max_timeout_ms=max_timeout_ms,
        )

    async def _get_tool(self) -> ViewImageTool:
        if self._resolved is None:
            tool = await ViewImageTool.create(self._root, **self._limits)
            # All racing first calls use the first installed native instance,
            # including its worker quota. No event-loop-bound lock is needed.
            if self._resolved is None:
                self._resolved = tool
        return self._resolved

    async def handler(self, name: str, arguments: dict[str, Any]) -> list[Any]:
        """Return AITextPart metadata and an AIImagePart carrying real pixels."""
        from roomkit.core.exceptions import ToolFailedError, ToolRefusedError, UnservedToolCallError
        from roomkit.providers.ai import AIImagePart, AITextPart

        if name != "view_image":
            raise UnservedToolCallError(name)
        try:
            result = await self._call(arguments)
        except (InvalidInputError, AccessRefusedError) as error:
            raise ToolRefusedError(str(error)) from error
        except (LimitExceededError, UnsupportedResourceError) as error:
            raise ToolFailedError(str(error)) from error
        metadata = dict(result["data"])
        image = metadata.pop("image")
        return [
            AITextPart(
                text=json.dumps({"data": metadata, "text": result["text"]}, ensure_ascii=False)
            ),
            AIImagePart(
                url=f"data:{image['mime_type']};base64,{image['base64']}",
                mime_type=image["mime_type"],
            ),
        ]


class Ocr(_ToolAdapter):
    """Select local OCR with an explicit, independently installed backend.

    Limits are the engine-independent host bounds; omitted ones use Rust
    defaults. The backend binds the root and validates them on first use.
    """

    _tool_type = OcrTool

    def __init__(
        self,
        root: str | os.PathLike[str],
        *,
        backend: OcrBackend,
        max_file_bytes: int | None = None,
        max_dimension: int | None = None,
        max_pixels: int | None = None,
        max_decode_bytes: int | None = None,
        max_output_bytes: int | None = None,
        max_blocks: int | None = None,
        default_timeout_ms: int | None = None,
        max_timeout_ms: int | None = None,
    ) -> None:
        path = os.fspath(root)
        if not path:
            raise InvalidInputError("root must not be empty")
        self._root = os.path.abspath(path)
        self._backend = backend
        self._limits = {
            "max_file_bytes": max_file_bytes,
            "max_dimension": max_dimension,
            "max_pixels": max_pixels,
            "max_decode_bytes": max_decode_bytes,
            "max_output_bytes": max_output_bytes,
            "max_blocks": max_blocks,
            "default_timeout_ms": default_timeout_ms,
            "max_timeout_ms": max_timeout_ms,
        }
        self._resolved: OcrTool | None = None

    async def _get_tool(self) -> OcrTool:
        if self._resolved is None:
            tool = await OcrTool.create(self._root, backend=self._backend, **self._limits)
            if self._resolved is None:
                self._resolved = tool
        return self._resolved
