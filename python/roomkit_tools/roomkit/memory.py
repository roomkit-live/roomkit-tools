"""Expose a host-bound memory as a RoomKit tool."""

from ..memory import MemoryTool
from ._adapter import _ToolAdapter


class Memory(_ToolAdapter[MemoryTool]):
    """The host binds the namespace and authorizes operations."""

    _tool_type = MemoryTool
