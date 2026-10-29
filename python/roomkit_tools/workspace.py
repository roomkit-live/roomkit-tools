"""Explicit host-owned scopes for tools that operate on the same directory."""

from __future__ import annotations

import os

from . import _native


class Workspace(os.PathLike[str]):
    """An absolute root and a shared Rust mutation lock, initialized lazily.

    Pass the same object as root to Write/Edit or WriteTool/EditTool to
    serialize their mutations. Read, List, Glob and Grep also accept it as a
    path; reads do not acquire the mutation lock. Construction does not resolve
    the directory or open files.
    The directory must exist on first use. Failures can be retried, and the
    object can be used across event loops. Separate Workspace objects, tools
    constructed from plain paths, and external editors do not share this lock.
    """

    def __init__(self, root: str | os.PathLike[str]) -> None:
        path = os.fspath(root)
        if not path:
            raise _native.InvalidInputError("root must not be empty")
        self._path = os.path.abspath(path)
        self._native = _native._Workspace(self._path)

    def __fspath__(self) -> str:
        """Return the absolute path captured at construction."""
        return self._path
