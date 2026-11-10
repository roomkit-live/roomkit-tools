"""RoomKit tools that delegate work to the calling room and agent's backend,
and ``ask_user``, served by RoomKit's human input."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from ..agents import (
    AgentFollowupTool,
    AgentScope,
    AgentSendTool,
    AskUserTool,
    CancelTaskTool,
    DelegateTaskTool,
    TaskStatusTool,
)
from ._adapter import _mapped, _structured, _ToolAdapter, _trusted_caller

if TYPE_CHECKING:
    from roomkit.tools import HumanInputToolHandler


class _AgentAdapter[T](_ToolAdapter[T]):
    async def handler(self, name: str, arguments: dict[str, Any]) -> str:
        """Resolve RoomKit's trusted caller; never accept scope from the model.

        A call outside a RoomKit tool call is refused; the result is also the
        call's structured content.
        """
        from roomkit.core.exceptions import UnservedToolCallError

        if name != self._tool.name:
            raise UnservedToolCallError(name)
        room, actor = _trusted_caller("agent")
        return _structured(await _mapped(self._call_as(room, actor, arguments)))

    async def _call_as(self, room: str, actor: str, arguments: dict[str, Any]) -> dict[str, Any]:
        return await self._tool.call(arguments, scope=AgentScope(room, actor))


class DelegateTask(_AgentAdapter[DelegateTaskTool]):
    """Select a built DelegateTaskTool in RoomKit's tools list: ``DelegateTask(tool)``.

    The model's profile choices are the backend's ``allowed_profiles``, when
    it declares them.
    """

    _tool_type = DelegateTaskTool

    @property
    def definition(self) -> dict[str, Any]:
        definition = super().definition
        profiles = getattr(self._tool._backend, "allowed_profiles", None)
        if profiles is not None:
            definition["parameters"]["properties"]["profile"]["enum"] = list(profiles)
        return definition


class TaskStatus(_AgentAdapter[TaskStatusTool]):
    """Select a built TaskStatusTool in RoomKit's tools list: ``TaskStatus(tool)``."""

    _tool_type = TaskStatusTool


class CancelTask(_AgentAdapter[CancelTaskTool]):
    """Select a built CancelTaskTool in RoomKit's tools list: ``CancelTask(tool)``."""

    _tool_type = CancelTaskTool


class AgentFollowup(_AgentAdapter[AgentFollowupTool]):
    """Select a built AgentFollowupTool in RoomKit's tools list: ``AgentFollowup(tool)``."""

    _tool_type = AgentFollowupTool


class AgentSend(_AgentAdapter[AgentSendTool]):
    """Select a built AgentSendTool in RoomKit's tools list: ``AgentSend(tool)``."""

    _tool_type = AgentSendTool


def ask_user_handler(timeout: float) -> HumanInputToolHandler:
    """``ask_user`` served by RoomKit's human input, each call waiting
    *timeout* seconds at most for the person's answers:
    ``AIChannel(..., human_input_handler=ask_user_handler(timeout))``.

    Each call opens a request and fires ``ON_USER_INPUT_REQUIRED``. The host
    reads its questions with ``AskUserTool.read(event.arguments)``, shows
    them, and settles the request with ``kit.answer_input``, or with
    ``kit.reject_input`` and the reason, which the model reads.
    """
    from roomkit.providers.ai.base import AITool
    from roomkit.tools import HumanInputToolHandler

    definition = AskUserTool.definition()
    declared = AITool(
        name=definition["name"],
        description=definition["description"],
        parameters=definition["input_schema"],
    )
    return HumanInputToolHandler({AskUserTool.name}, timeout=timeout, tool_definitions=[declared])
