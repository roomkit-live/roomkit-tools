"""RoomKit tools that manage plans in the calling room and agent's scope."""

from __future__ import annotations

from typing import Any

from ..plans import (
    PlanCreateTool,
    PlanDeleteTool,
    PlanListTool,
    PlanReadTool,
    PlanScope,
    PlanUpdateTool,
)
from ._adapter import _mapped, _structured, _ToolAdapter, _trusted_caller


class _PlanAdapter[T](_ToolAdapter[T]):
    async def handler(self, name: str, arguments: dict[str, Any]) -> str:
        """Use RoomKit's trusted conversation/channel identity for every call.

        A call outside a RoomKit tool call is refused; the result is also the
        call's structured content.
        """
        from roomkit.core.exceptions import UnservedToolCallError

        if name != self._tool.name:
            raise UnservedToolCallError(name)
        room, actor = _trusted_caller("plan")
        return _structured(await _mapped(self._call_as(room, actor, arguments)))

    async def _call_as(self, room: str, actor: str, arguments: dict[str, Any]) -> dict[str, Any]:
        return await self._tool.call(arguments, scope=PlanScope(room, actor))


class PlanCreate(_PlanAdapter[PlanCreateTool]):
    """Select a built PlanCreateTool in RoomKit's tools list: ``PlanCreate(tool)``."""

    _tool_type = PlanCreateTool


class PlanRead(_PlanAdapter[PlanReadTool]):
    """Select a built PlanReadTool in RoomKit's tools list: ``PlanRead(tool)``."""

    _tool_type = PlanReadTool


class PlanList(_PlanAdapter[PlanListTool]):
    """Select a built PlanListTool in RoomKit's tools list: ``PlanList(tool)``."""

    _tool_type = PlanListTool


class PlanUpdate(_PlanAdapter[PlanUpdateTool]):
    """Select a built PlanUpdateTool in RoomKit's tools list: ``PlanUpdate(tool)``."""

    _tool_type = PlanUpdateTool


class PlanDelete(_PlanAdapter[PlanDeleteTool]):
    """Select a built PlanDeleteTool in RoomKit's tools list: ``PlanDelete(tool)``."""

    _tool_type = PlanDeleteTool
