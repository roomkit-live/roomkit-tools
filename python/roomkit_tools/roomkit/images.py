"""RoomKit tools that view or transcribe local images."""

from __future__ import annotations

import json
from typing import Any

from ..image import ViewImageTool
from ..ocr import OcrTool
from ._adapter import _mapped, _ToolAdapter


class ViewImage(_ToolAdapter[ViewImageTool]):
    """Select local image pixels as RoomKit multimodal tool output.

    ``ViewImage(await ViewImageTool.create(root))``. Register with a
    vision-capable model. The handler returns typed text/image parts, never
    base64 in ordinary model text. Static PNG/JPEG/WebP only; no OCR or
    remote URL fetching.
    """

    _tool_type = ViewImageTool

    async def handler(self, name: str, arguments: dict[str, Any]) -> list[Any]:
        """Return AITextPart metadata and an AIImagePart carrying real pixels."""
        from roomkit.core.exceptions import UnservedToolCallError
        from roomkit.providers.ai import AIImagePart, AITextPart

        if name != "view_image":
            raise UnservedToolCallError(name)
        result = await _mapped(self._tool.call(arguments))
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


class Ocr(_ToolAdapter[OcrTool]):
    """Select local OCR, built with an explicit, independently installed backend.

    ``Ocr(await OcrTool.create(root, backend=backend))``.
    """

    _tool_type = OcrTool
