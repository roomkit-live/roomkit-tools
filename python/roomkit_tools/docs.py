"""Host-configured documentation with native contracts and optional file storage."""

from __future__ import annotations

import json
import os
from typing import Any, ClassVar, Protocol

from ._native import (
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

    def __init__(
        self,
        root: str | os.PathLike[str],
        *,
        version: str | None = None,
        max_documents: int | None = None,
        max_document_bytes: int | None = None,
        max_total_bytes: int | None = None,
        max_entries: int | None = None,
        max_depth: int | None = None,
        max_total_lines: int | None = None,
        max_read_bytes: int | None = None,
    ):
        """Capture the root; omitted bounds use Rust defaults.

        Defaults: 256 documents of at most 256 KiB, 8 MiB in all, 4096 scanned
        entries, 16 nested directories, 65536 lines and 32 KiB read pages.
        """
        limits = {
            name: value
            for name, value in {
                "max_documents": max_documents,
                "max_document_bytes": max_document_bytes,
                "max_total_bytes": max_total_bytes,
                "max_entries": max_entries,
                "max_depth": max_depth,
                "max_total_lines": max_total_lines,
                "max_read_bytes": max_read_bytes,
            }.items()
            if value is not None
        }
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
        self, *, offset: int = 1, limit: int = 20, expected_catalog_revision: str | None = None
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
        offset: int = 1,
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
    def definition(cls) -> dict[str, Any]:
        """Input/output schemas derived from actual Rust types."""
        return json.loads(_DocsContract.definition_json(cls.name))

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


class DocsListTool(_DocsTool):
    """Discover document IDs, titles, sources and revisions."""

    name = "docs_list"
    method = "list"


class DocsSearchTool(_DocsTool):
    """Search documentation and return ranked, attributable source excerpts."""

    name = "docs_search"
    method = "search"


class DocsReadTool(_DocsTool):
    """Read complete numbered lines with optional revision checking."""

    name = "docs_read"
    method = "read"
