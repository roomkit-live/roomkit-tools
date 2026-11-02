"""RoomKit tools that read, search and compare files under a root."""

from __future__ import annotations

from ..code import CodeOutlineTool
from ..fs import DiffTool, GlobTool, GrepTool, ListTool, ReadTool
from ._adapter import _ToolAdapter


class CodeOutline(_ToolAdapter[CodeOutlineTool]):
    """Select Rust/Python syntax outlines in RoomKit's tools list.

    Takes a built tool: ``CodeOutline(await CodeOutlineTool.create(root))``.
    """

    _tool_type = CodeOutlineTool


class Read(_ToolAdapter[ReadTool]):
    """A built ReadTool, ready to pass to RoomKit's tools list.

    ``Read(await ReadTool.create(root, max_lines=200))``. RoomKit owns schema
    validation, policies, hooks and turn cancellation. I/O errors propagate
    for RoomKit to report safely; explicit refusals and resource failures
    expose their public messages. RoomKit is imported only by the handler.
    """

    _tool_type = ReadTool


class List(_ToolAdapter[ListTool]):
    """A built ListTool, selectable in RoomKit's tools list.

    See ListTool for its behavior and default bounds. Registering it does not
    expose other tools.
    """

    _tool_type = ListTool


class Glob(_ToolAdapter[GlobTool]):
    """A built GlobTool, selectable in RoomKit's tools list.

    See GlobTool for search behavior and default bounds. Registering it does
    not expose other tools.
    """

    _tool_type = GlobTool


class Grep(_ToolAdapter[GrepTool]):
    """A built GrepTool, selectable in RoomKit's tools list.

    See GrepTool for search behavior and default bounds. Registering it does
    not expose other tools.
    """

    _tool_type = GrepTool


class Diff(_ToolAdapter[DiffTool]):
    """A built DiffTool, selectable in RoomKit's tools list.

    See DiffTool for defaults and comparison semantics. This tool performs no
    mutations.
    """

    _tool_type = DiffTool
