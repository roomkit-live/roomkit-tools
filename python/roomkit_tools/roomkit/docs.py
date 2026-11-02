"""RoomKit tools that read host-configured documentation."""

from __future__ import annotations

from typing import Any

from ..docs import DocsListTool, DocsReadTool, DocsSearchTool
from ._adapter import _mapped, _structured, _ToolAdapter


class _DocsAdapter[T](_ToolAdapter[T]):
    async def handler(self, name: str, arguments: dict[str, Any]) -> str:
        """RoomKit adapter; the host selects the documentation accessible here.

        Refusals and resource failures expose their public messages. I/O and
        other tool errors propagate for RoomKit to report without host details.
        The result is also the call's structured content.
        """
        from roomkit.core.exceptions import UnservedToolCallError

        if name != self._tool.name:
            raise UnservedToolCallError(name)
        return _structured(await _mapped(self._tool.call(arguments)))


class DocsList(_DocsAdapter[DocsListTool]):
    """Select a built DocsListTool in RoomKit's tools list: ``DocsList(tool)``."""

    _tool_type = DocsListTool


class DocsSearch(_DocsAdapter[DocsSearchTool]):
    """Select a built DocsSearchTool in RoomKit's tools list: ``DocsSearch(tool)``."""

    _tool_type = DocsSearchTool


class DocsRead(_DocsAdapter[DocsReadTool]):
    """Select a built DocsReadTool in RoomKit's tools list: ``DocsRead(tool)``."""

    _tool_type = DocsReadTool
