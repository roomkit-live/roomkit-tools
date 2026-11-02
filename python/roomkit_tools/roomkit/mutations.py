"""RoomKit tools that change files under a shared mutation workspace."""

from __future__ import annotations

from ..fs import EditTool, PatchTool, WriteTool
from ._adapter import _ToolAdapter


class Write(_ToolAdapter[WriteTool]):
    """A built WriteTool, selectable in RoomKit's tools list.

    Build Write, Edit and Patch from the same Workspace to serialize their
    mutations: ``Write(await WriteTool.create(workspace))``. See WriteTool for
    overwrite/revision and cancellation semantics.
    """

    _tool_type = WriteTool


class Edit(_ToolAdapter[EditTool]):
    """A built EditTool, selectable in RoomKit's tools list.

    Build Write and Edit from the same Workspace to serialize their mutations.
    See EditTool for matching rules; a cancelled commit may still finish.
    """

    _tool_type = EditTool


class Patch(_ToolAdapter[PatchTool]):
    """A built PatchTool, selectable in RoomKit's tools list.

    Build Patch, Write and Edit from the same Workspace for shared
    serialization. dry_run defaults to true. Inspect completed, applied and
    failure before reporting success; a batch has no multi-file rollback. See
    PatchTool for bounds and exact-content checks.
    """

    _tool_type = PatchTool
