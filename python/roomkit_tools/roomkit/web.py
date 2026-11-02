"""RoomKit tools that fetch and search the web."""

from __future__ import annotations

from ..search import WebSearchTool
from ..web import WebFetchTool
from ._adapter import _ToolAdapter


class WebFetch(_ToolAdapter[WebFetchTool]):
    """A built WebFetchTool, with its host origin rules, in RoomKit's tools list.

    ``WebFetch(WebFetchTool(allowed_origins=[...]))``. Construction performs no I/O.
    """

    _tool_type = WebFetchTool


class WebSearch(_ToolAdapter[WebSearchTool]):
    """A built WebSearchTool in RoomKit's tools list.

    ``WebSearch(WebSearchTool(backend=ExaSearch()))``.
    """

    _tool_type = WebSearchTool
