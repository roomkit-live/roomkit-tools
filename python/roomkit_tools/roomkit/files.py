"""RoomKit tools that read, search and compare files under a root."""

from __future__ import annotations

import os

from ..code import CodeOutlineTool
from ..fs import DiffTool, GlobTool, GrepTool, ListTool, ReadTool
from ._adapter import _ConfiguredTool, _ToolAdapter


class ReadToolAdapter(_ToolAdapter):
    """Register an already-created ReadTool with a RoomKit AI channel.

    RoomKit owns schema validation, policies, hooks and turn cancellation.
    I/O errors propagate for RoomKit to report safely; explicit refusals and
    resource failures expose their public messages. No background job is started.
    """

    _tool_type = ReadTool

    def __init__(self, tool: ReadTool) -> None:
        self._tool = tool

    async def _get_tool(self) -> ReadTool:
        return self._tool


class CodeOutline(_ConfiguredTool):
    """Select Rust/Python syntax outlines directly in RoomKit's tools list.

    Configuration uses CodeOutlineTool.create's host limits. Construction
    captures the root; the first call resolves it and validates limits in Rust.
    """

    _tool_type = CodeOutlineTool

    def __init__(
        self,
        root: str | os.PathLike[str],
        *,
        max_file_bytes: int | None = None,
        max_output_bytes: int | None = None,
        max_symbols: int | None = None,
        max_scan_symbols: int | None = None,
        max_scan_nodes: int | None = None,
        max_depth: int | None = None,
        max_signature_bytes: int | None = None,
        default_timeout_ms: int | None = None,
        max_timeout_ms: int | None = None,
    ) -> None:
        self._configure(
            root,
            max_file_bytes=max_file_bytes,
            max_output_bytes=max_output_bytes,
            max_symbols=max_symbols,
            max_scan_symbols=max_scan_symbols,
            max_scan_nodes=max_scan_nodes,
            max_depth=max_depth,
            max_signature_bytes=max_signature_bytes,
            default_timeout_ms=default_timeout_ms,
            max_timeout_ms=max_timeout_ms,
        )


class Read(_ConfiguredTool):
    """A Rust read tool ready to pass directly to RoomKit's tools list.

    Construction captures an absolute root without opening files. The first
    handler call resolves it and validates limits asynchronously in Rust.
    Initialization failures can be retried. RoomKit is imported only by the
    handler. Use ReadTool for the standalone dictionary-returning API.
    """

    _tool_type = ReadTool

    def __init__(
        self,
        root: str | os.PathLike[str],
        *,
        max_file_bytes: int | None = None,
        max_output_bytes: int | None = None,
        max_lines: int | None = None,
    ) -> None:
        self._configure(
            root,
            max_file_bytes=max_file_bytes,
            max_output_bytes=max_output_bytes,
            max_lines=max_lines,
        )


class List(_ConfiguredTool):
    """The Rust list tool, selectable directly in RoomKit's tools list.

    The root and limits are resolved asynchronously on first use, as for Read.
    See ListTool for search behavior and default bounds. Each instance owns
    its configuration; registering it does not expose other tools.
    """

    _tool_type = ListTool

    def __init__(
        self,
        root: str | os.PathLike[str],
        *,
        max_entries: int | None = None,
        max_output_bytes: int | None = None,
        max_scan_entries: int | None = None,
    ) -> None:
        self._configure(
            root,
            max_entries=max_entries,
            max_output_bytes=max_output_bytes,
            max_scan_entries=max_scan_entries,
        )


class Glob(_ConfiguredTool):
    """The Rust glob tool, selectable directly in RoomKit's tools list.

    The root and limits are resolved asynchronously on first use, as for Read.
    See GlobTool for search behavior and default bounds. Each instance owns
    its configuration; registering it does not expose other tools.
    """

    _tool_type = GlobTool

    def __init__(
        self,
        root: str | os.PathLike[str],
        *,
        max_matches: int | None = None,
        max_output_bytes: int | None = None,
        max_scan_entries: int | None = None,
        max_depth: int | None = None,
    ) -> None:
        self._configure(
            root,
            max_matches=max_matches,
            max_output_bytes=max_output_bytes,
            max_scan_entries=max_scan_entries,
            max_depth=max_depth,
        )


class Grep(_ConfiguredTool):
    """The Rust grep tool, selectable directly in RoomKit's tools list.

    The root and limits are resolved asynchronously on first use, as for Read.
    See GrepTool for search behavior and default bounds. Each instance owns
    its configuration; registering it does not expose other tools.
    """

    _tool_type = GrepTool

    def __init__(
        self,
        root: str | os.PathLike[str],
        *,
        max_matches: int | None = None,
        max_output_bytes: int | None = None,
        max_scan_entries: int | None = None,
        max_depth: int | None = None,
        max_file_bytes: int | None = None,
        max_total_bytes: int | None = None,
        max_context_lines: int | None = None,
        max_line_bytes: int | None = None,
    ) -> None:
        self._configure(
            root,
            max_matches=max_matches,
            max_output_bytes=max_output_bytes,
            max_scan_entries=max_scan_entries,
            max_depth=max_depth,
            max_file_bytes=max_file_bytes,
            max_total_bytes=max_total_bytes,
            max_context_lines=max_context_lines,
            max_line_bytes=max_line_bytes,
        )


class Diff(_ConfiguredTool):
    """The Rust file comparison tool, selectable directly in RoomKit's tools list.

    The root and limits resolve asynchronously on first use. See DiffTool for
    defaults and comparison semantics. This tool performs no mutations.
    """

    _tool_type = DiffTool

    def __init__(
        self,
        root: str | os.PathLike[str],
        *,
        max_file_bytes: int | None = None,
        max_lines: int | None = None,
        max_comparison_cells: int | None = None,
        max_output_bytes: int | None = None,
        max_context_lines: int | None = None,
    ) -> None:
        self._configure(
            root,
            max_file_bytes=max_file_bytes,
            max_lines=max_lines,
            max_comparison_cells=max_comparison_cells,
            max_output_bytes=max_output_bytes,
            max_context_lines=max_context_lines,
        )
