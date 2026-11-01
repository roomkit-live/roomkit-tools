"""Optional adapters for RoomKit Python's public Tool protocol."""

from .commands import (
    OutputForget,
    OutputRead,
    ProcessRead,
    ProcessStart,
    ProcessStatus,
    ProcessStop,
    ProcessWrite,
    Shell,
)
from .files import CodeOutline, Diff, Glob, Grep, List, Read, ReadToolAdapter
from .images import Ocr, ViewImage
from .mutations import Edit, Patch, Write
from .web import WebFetch, WebSearch

__all__ = [
    "CodeOutline",
    "Diff",
    "Edit",
    "Glob",
    "Grep",
    "List",
    "Ocr",
    "OutputForget",
    "OutputRead",
    "Patch",
    "ProcessRead",
    "ProcessStart",
    "ProcessStatus",
    "ProcessStop",
    "ProcessWrite",
    "Read",
    "ReadToolAdapter",
    "Shell",
    "ViewImage",
    "WebFetch",
    "WebSearch",
    "Write",
]
