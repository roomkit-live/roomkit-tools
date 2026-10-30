"""Optional adapters for RoomKit Python's public Tool protocol."""

from __future__ import annotations

import json
import os
from typing import Any, ClassVar

from ._native import (
    AccessRefusedError,
    InvalidInputError,
    LimitExceededError,
    UnsupportedResourceError,
)
from .code import CodeOutlineTool
from .fs import DiffTool, EditTool, GlobTool, GrepTool, ListTool, PatchTool, ReadTool, WriteTool
from .image import ViewImageTool
from .output import OutputStore
from .process import ProcessSession, ShellTool
from .search import SearchBackend, WebSearchTool
from .web import WebFetchTool
from .workspace import Workspace

type _BoundTool = (
    CodeOutlineTool
    | ViewImageTool
    | ReadTool
    | ListTool
    | GlobTool
    | GrepTool
    | WriteTool
    | EditTool
    | DiffTool
    | PatchTool
    | ShellTool
    | WebFetchTool
    | WebSearchTool
)


class _ToolAdapter:
    _tool_type: ClassVar[type[_BoundTool]]

    @property
    def definition(self) -> dict[str, Any]:
        """Expose the unchanged Rust input schema under RoomKit's parameters key."""
        definition = self._tool_type.definition()
        return {
            "name": definition["name"],
            "description": definition["description"],
            "parameters": definition["input_schema"],
        }

    async def handler(self, name: str, arguments: dict[str, Any]) -> str:
        """Return data and text, preserving refusal, failure and cancellation."""
        from roomkit.core.exceptions import ToolFailedError, ToolRefusedError, UnservedToolCallError

        if name != self.definition["name"]:
            raise UnservedToolCallError(name)
        try:
            result = await self._call(arguments)
        except (InvalidInputError, AccessRefusedError) as error:
            raise ToolRefusedError(str(error)) from error
        except (LimitExceededError, UnsupportedResourceError) as error:
            raise ToolFailedError(str(error)) from error
        return json.dumps(result, ensure_ascii=False)

    async def _call(self, arguments: dict[str, Any]) -> dict[str, Any]:
        tool = await self._get_tool()
        return await tool.call(arguments)

    async def _get_tool(self) -> _BoundTool:
        raise NotImplementedError


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


class _ConfiguredTool(_ToolAdapter):
    def _configure(self, root: str | os.PathLike[str], **limits: int | None) -> None:
        path = os.fspath(root)
        if not path:
            raise InvalidInputError("root must not be empty")
        self._root = os.path.abspath(path)
        self._limits = limits
        self._resolved: _BoundTool | None = None

    async def _get_tool(self) -> _BoundTool:
        if self._resolved is None:
            # Each first caller owns its initialization: cancellation cannot
            # poison another call, and no task/lock is tied to an event loop.
            tool = await self._tool_type.create(self._root, **self._limits)
            self._resolved = tool
        return self._resolved


class ViewImage(_ConfiguredTool):
    """Select local image pixels as RoomKit multimodal tool output.

    Register with a vision-capable model. The handler returns typed text/image
    parts, never base64 in ordinary model text. Limits are ViewImageTool.create's
    host options. Static PNG/JPEG/WebP only; no OCR or remote URL fetching.
    """

    _tool_type = ViewImageTool

    def __init__(self, root: str | os.PathLike[str], **limits: int | None) -> None:
        self._configure(root, **limits)

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


class CodeOutline(_ConfiguredTool):
    """Select Rust/Python syntax outlines directly in RoomKit's tools list.

    Configuration uses CodeOutlineTool.create's host limits. Construction
    captures the root; the first call resolves it and validates limits in Rust.
    """

    _tool_type = CodeOutlineTool

    def __init__(self, root: str | os.PathLike[str], **limits: int | None) -> None:
        self._configure(root, **limits)


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
        )


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


class Shell(_ConfiguredTool):
    """The Rust foreground shell, selectable directly in RoomKit's tools list.

    Captures an absolute root without spawning. First use resolves the root and
    validates limits asynchronously. A Workspace is accepted as a path only;
    shell commands do not share its mutation lock or filesystem restrictions.
    See ShellTool for host permissions, bounds and cancellation semantics.
    """

    _tool_type = ShellTool

    def __init__(
        self,
        root: str | os.PathLike[str],
        *,
        outputs: OutputStore | None = None,
        max_command_bytes: int | None = None,
        max_output_bytes: int | None = None,
        default_timeout_ms: int | None = None,
        max_timeout_ms: int | None = None,
    ) -> None:
        self._configure(
            root,
            outputs=outputs,
            max_command_bytes=max_command_bytes,
            max_output_bytes=max_output_bytes,
            default_timeout_ms=default_timeout_ms,
            max_timeout_ms=max_timeout_ms,
        )


class _ProcessAdapter(_ToolAdapter):
    _name: ClassVar[str]

    def __init__(self, session: ProcessSession) -> None:
        if not isinstance(session, ProcessSession):
            raise TypeError("process tools require a shared ProcessSession")
        self._session = session

    @property
    def definition(self) -> dict[str, Any]:
        """Expose this capability only, using the original Rust input schema."""
        definition = ProcessSession.definition(self._name)
        return {
            "name": definition["name"],
            "description": definition["description"],
            "parameters": definition["input_schema"],
        }

    async def _call(self, arguments: dict[str, Any]) -> dict[str, Any]:
        return await self._session.call(self._name, arguments)


class ProcessStart(_ProcessAdapter):
    """Start persistent jobs in a host-selected ProcessSession.

    Pass the same session to selected process tools and close it at conversation
    shutdown. Python's Tool protocol supplies no room identity: create a separate
    session for each conversation/agent that must be isolated.
    """

    _name = "process_start"


class ProcessStatus(_ProcessAdapter):
    """Inspect retained jobs in the supplied ProcessSession without changing them."""

    _name = "process_status"


class ProcessRead(_ProcessAdapter):
    """Read available output in a ProcessSession; never writes or closes stdin."""

    _name = "process_read"


class ProcessWrite(_ProcessAdapter):
    """Write exact text or explicit EOF in a ProcessSession; partial writes are reported."""

    _name = "process_write"


class ProcessStop(_ProcessAdapter):
    """Stop and optionally forget a retained job in the supplied ProcessSession."""

    _name = "process_stop"


class _OutputAdapter(_ToolAdapter):
    _name: ClassVar[str]

    def __init__(self, store: OutputStore) -> None:
        if not isinstance(store, OutputStore):
            raise TypeError("output tools require an OutputStore")
        self._store = store

    @property
    def definition(self) -> dict[str, Any]:
        definition = OutputStore.definition(self._name)
        return {
            "name": definition["name"],
            "description": definition["description"],
            "parameters": definition["input_schema"],
        }

    async def _call(self, arguments: dict[str, Any]) -> dict[str, Any]:
        return await self._store.call(self._name, arguments)


class OutputRead(_OutputAdapter):
    """Read an owned disk copy by ID; register independently from output deletion."""

    _name = "output_read"


class OutputForget(_OutputAdapter):
    """Explicitly release finished output archives from the selected store."""

    _name = "output_forget"


class WebFetch(_ToolAdapter):
    """Select HTTP fetching independently, with host-configured origin rules.

    Keyword arguments are the same as WebFetchTool. Construction performs no I/O.
    """

    _tool_type = WebFetchTool

    def __init__(self, **configuration: Any) -> None:
        self._tool = WebFetchTool(**configuration)

    async def _get_tool(self) -> WebFetchTool:
        return self._tool


class WebSearch(_ToolAdapter):
    """Pass WebSearch(backend=ExaSearch()) directly to RoomKit's tools list."""

    _tool_type = WebSearchTool

    def __init__(self, *, backend: SearchBackend, **configuration: Any) -> None:
        self._tool = WebSearchTool(backend=backend, **configuration)

    async def _get_tool(self) -> WebSearchTool:
        return self._tool
