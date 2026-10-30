"""Host-configured documentation with native contracts and optional file storage."""

from __future__ import annotations

import json
import os
from typing import Any, ClassVar, Protocol

from ._native import (
    AccessRefusedError,
    InvalidInputError,
    ToolError,
    UnsupportedResourceError,
    _Cancellation,
    _DocsContract,
    _FileDocs,
)


class DocsBackend(Protocol):
    """Injected documentation source returning data dictionaries, not envelopes.

    Implement bounded search/read, stable IDs, provenance, cancellation and revision
    checks. Documentation never grants tools or overrides the host's instructions.
    """

    async def list(
        self, *, offset: int, limit: int, expected_catalog_revision: str | None
    ) -> dict[str, Any]: ...
    async def search(
        self, *, query: str, offset: int, limit: int, expected_catalog_revision: str | None
    ) -> dict[str, Any]: ...
    async def read(
        self, *, document_id: str, offset: int, limit: int, expected_revision: str | None
    ) -> dict[str, Any]: ...


class FileDocs:
    """Lazy native Markdown/text snapshot; recreate this backend to refresh.

    Hidden names and symlinks are excluded. The directory must be host-controlled.
    Instances own independent snapshots; selected tools share this backend explicitly.
    """

    def __init__(self, root: str | os.PathLike[str], *, version: str | None = None, **limits: int):
        self._native = _FileDocs(
            os.path.abspath(root), version, json.dumps(limits, allow_nan=False)
        )

    async def _call(self, name: str, arguments: dict[str, Any]) -> dict[str, Any]:
        token = _Cancellation()
        try:
            result = await self._native.call(name, json.dumps(arguments, allow_nan=False), token)
            return json.loads(result)
        finally:
            token.cancel()

    async def list(
        self, *, offset: int = 0, limit: int = 20, expected_catalog_revision: str | None = None
    ) -> dict[str, Any]:
        return await self._call(
            "docs_list",
            {
                "offset": offset,
                "limit": limit,
                "expected_catalog_revision": expected_catalog_revision,
            },
        )

    async def search(
        self,
        *,
        query: str,
        offset: int = 0,
        limit: int = 10,
        expected_catalog_revision: str | None = None,
    ) -> dict[str, Any]:
        return await self._call(
            "docs_search",
            {
                "query": query,
                "offset": offset,
                "limit": limit,
                "expected_catalog_revision": expected_catalog_revision,
            },
        )

    async def read(
        self,
        *,
        document_id: str,
        offset: int = 1,
        limit: int = 100,
        expected_revision: str | None = None,
    ) -> dict[str, Any]:
        return await self._call(
            "docs_read",
            {
                "document_id": document_id,
                "offset": offset,
                "limit": limit,
                "expected_revision": expected_revision,
            },
        )


class _DocsTool:
    name: ClassVar[str]
    method: ClassVar[str]

    def __init__(self, backend: DocsBackend):
        self._backend = backend

    @classmethod
    def contract(cls) -> dict[str, Any]:
        """Input/output schemas derived from actual Rust types."""
        return json.loads(_DocsContract.definition_json(cls.name))

    @property
    def definition(self) -> dict[str, Any]:
        """RoomKit tool definition for explicit selection."""
        contract = self.contract()
        return {
            "name": contract["name"],
            "description": contract["description"],
            "parameters": contract["input_schema"],
        }

    async def call(self, arguments: dict[str, Any]) -> dict[str, Any]:
        """Validate arguments and backend output; return the common envelope."""
        inputs = json.loads(
            _DocsContract.input_json(self.name, json.dumps(arguments, allow_nan=False))
        )
        method = getattr(self._backend, self.method, None)
        if method is None:
            raise UnsupportedResourceError(f"backend does not support {self.name}")
        data = await method(**inputs)
        return json.loads(_DocsContract.output_json(self.name, json.dumps(data, allow_nan=False)))

    async def handler(self, name: str, arguments: dict[str, Any]) -> str:
        """RoomKit adapter; the host selects the documentation accessible here."""
        from roomkit.core.exceptions import ToolFailedError, ToolRefusedError, UnservedToolCallError
        from roomkit.tools.context import current_tool_call

        if name != self.name:
            raise UnservedToolCallError(name)
        try:
            result = await self.call(arguments)
        except (InvalidInputError, AccessRefusedError) as error:
            raise ToolRefusedError(str(error)) from error
        except ToolError as error:
            raise ToolFailedError(str(error)) from error
        context = current_tool_call()
        if context is not None:
            context.structured_content = result
        return json.dumps(result, ensure_ascii=False)


class DocsList(_DocsTool):
    """Discover document IDs, titles, sources and revisions."""

    name = "docs_list"
    method = "list"


class DocsSearch(_DocsTool):
    """Search documentation and return ranked, attributable source excerpts."""

    name = "docs_search"
    method = "search"


class DocsRead(_DocsTool):
    """Read complete numbered lines with optional revision checking."""

    name = "docs_read"
    method = "read"
