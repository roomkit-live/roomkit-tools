"""Optional adapters for RoomKit Python's public Tool protocol."""

from .agents import (
    AgentFollowup,
    AgentSend,
    CancelTask,
    DelegateTask,
    TaskStatus,
    ask_user_handler,
)
from .commands import (
    OutputForget,
    OutputRead,
    ProcessRead,
    ProcessStart,
    ProcessStatus,
    ProcessStop,
    ProcessWrite,
    Shell,
    roomkit_owner,
)
from .docs import DocsList, DocsRead, DocsSearch
from .ends import notify_job_ends
from .files import CodeOutline, Diff, Glob, Grep, List, Read
from .images import Ocr, ViewImage
from .mutations import Edit, Patch, Write
from .plans import PlanCreate, PlanDelete, PlanList, PlanRead, PlanUpdate
from .web import WebFetch, WebSearch

__all__ = [
    "AgentFollowup",
    "AgentSend",
    "CancelTask",
    "CodeOutline",
    "DelegateTask",
    "Diff",
    "DocsList",
    "DocsRead",
    "DocsSearch",
    "Edit",
    "Glob",
    "Grep",
    "List",
    "Ocr",
    "OutputForget",
    "OutputRead",
    "Patch",
    "PlanCreate",
    "PlanDelete",
    "PlanList",
    "PlanRead",
    "PlanUpdate",
    "ProcessRead",
    "ProcessStart",
    "ProcessStatus",
    "ProcessStop",
    "ProcessWrite",
    "Read",
    "Shell",
    "TaskStatus",
    "ViewImage",
    "WebFetch",
    "WebSearch",
    "Write",
    "ask_user_handler",
    "notify_job_ends",
    "roomkit_owner",
]
