"""One project memory tool over an explicitly bound backend."""

from __future__ import annotations

import json
import os
from typing import Any, Protocol

from ._native import _Cancellation, _FileMemory, _MemoryContract


class MemoryBackend(Protocol):
    """The host binds one namespace and enforces its storage quota."""

    async def execute(self, arguments: dict[str, Any]) -> dict[str, Any]: ...


class FileMemory:
    """A bounded Markdown directory shared by cooperating local processes."""

    def __init__(self, root: str | os.PathLike[str], max_bytes: int):
        self._native = _FileMemory(os.path.abspath(root), max_bytes)

    async def execute(self, arguments: dict[str, Any]) -> dict[str, Any]:
        token = _Cancellation()
        try:
            return json.loads(await self._native.execute(json.dumps(arguments), token))
        finally:
            token.cancel()


class MemoryTool:
    """Search, read, save or forget project notes using the configured backend."""

    def __init__(self, backend: MemoryBackend):
        self._backend = backend

    @staticmethod
    def definition() -> dict[str, Any]:
        return json.loads(_MemoryContract.definition())

    @staticmethod
    def validate_arguments(arguments: dict[str, Any]) -> None:
        """Reject invalid arguments before calling an external backend."""
        _MemoryContract.input(json.dumps(arguments, allow_nan=False))

    @staticmethod
    def validate_result(arguments: dict[str, Any], result: dict[str, Any]) -> None:
        """Reject invalid or unrelated records returned by a backend."""
        _MemoryContract.output(
            json.dumps(arguments, allow_nan=False), json.dumps(result, allow_nan=False)
        )

    async def call(self, arguments: dict[str, Any]) -> dict[str, Any]:
        raw = _MemoryContract.input(json.dumps(arguments, allow_nan=False))
        data = await self._backend.execute(json.loads(raw))
        return json.loads(_MemoryContract.output(raw, json.dumps(data, allow_nan=False)))
