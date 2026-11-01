"""RoomKit tools that change files under a shared mutation workspace."""

from __future__ import annotations

import os
from typing import ClassVar

from ..fs import EditTool, PatchTool, WriteTool
from ..workspace import Workspace
from ._adapter import _ToolAdapter


class _ConfiguredMutation(_ToolAdapter):
    _tool_type: ClassVar[type[WriteTool | EditTool | PatchTool]]

    def __init__(self, root: str | os.PathLike[str], *, max_file_bytes: int | None = None) -> None:
        self._configure_mutation(root, max_file_bytes=max_file_bytes)

    def _configure_mutation(self, root: str | os.PathLike[str], **limits: int | None) -> None:
        # Retain one scope even when multiple first calls initialize concurrently.
        self._workspace = root if isinstance(root, Workspace) else Workspace(root)
        self._limits = limits
        self._resolved: WriteTool | EditTool | PatchTool | None = None

    async def _get_tool(self) -> WriteTool | EditTool | PatchTool:
        if self._resolved is None:
            tool = await self._tool_type.create(self._workspace, **self._limits)
            self._resolved = tool
        return self._resolved


class Write(_ConfiguredMutation):
    """The Rust write tool, selectable directly in RoomKit's tools list.

    Pass the same Workspace to Write and Edit to serialize their mutations.
    A plain path creates an independent scope. Root resolution and limit
    validation happen asynchronously on first use; max_file_bytes defaults to
    8 MiB. See WriteTool for overwrite/revision and cancellation semantics.
    """

    _tool_type = WriteTool


class Edit(_ConfiguredMutation):
    """The Rust exact-text edit tool, selectable directly in RoomKit's tools list.

    Pass the same Workspace to Write and Edit to serialize their mutations.
    max_file_bytes defaults to 8 MiB. The root and limits are resolved on first
    use. See EditTool for matching rules; a cancelled commit may still finish.
    """

    _tool_type = EditTool


class Patch(_ConfiguredMutation):
    """The Rust structured patch tool, selectable directly in RoomKit's tools list.

    Pass the same Workspace to Patch, Write and Edit for shared serialization.
    dry_run defaults to true. Inspect completed, applied and failure before
    reporting success; a batch has no multi-file rollback. See PatchTool for
    bounds and exact-content checks. Initialization is lazy and retryable.
    """

    _tool_type = PatchTool

    def __init__(
        self,
        root: str | os.PathLike[str],
        *,
        max_changes: int | None = None,
        max_input_bytes: int | None = None,
        max_file_bytes: int | None = None,
        max_lines: int | None = None,
        max_comparison_cells: int | None = None,
        max_output_bytes: int | None = None,
        max_context_lines: int | None = None,
    ) -> None:
        self._configure_mutation(
            root,
            max_changes=max_changes,
            max_input_bytes=max_input_bytes,
            max_file_bytes=max_file_bytes,
            max_lines=max_lines,
            max_comparison_cells=max_comparison_cells,
            max_output_bytes=max_output_bytes,
            max_context_lines=max_context_lines,
        )
