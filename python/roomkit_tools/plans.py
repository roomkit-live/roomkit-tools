"""Selectable plan CRUD with Rust contracts and explicitly injected storage."""

from __future__ import annotations

import json
from dataclasses import dataclass
from typing import Any, ClassVar, Protocol

from ._native import (
    AccessRefusedError,
    InvalidInputError,
    LimitExceededError,
    UnsupportedResourceError,
    _Cancellation,
    _MemoryPlanBackend,
    _PlanContract,
)


@dataclass(frozen=True)
class PlanScope:
    """Trusted conversation and agent identity; never selected by the model."""

    room_id: str
    caller_id: str

    def __post_init__(self) -> None:
        _PlanContract.scope(self.room_id, self.caller_id)


class PlanBackend(Protocol):
    """Host storage. Enforce scope, atomic revisions and bounds on every call.

    Return data dictionaries matching the Rust schemas. IDs must not be reused.
    Cancellation can lose a committed receipt: recover with read/list. Document
    durability; updates/deletions never execute or cancel related agent work.
    """

    async def create(
        self, scope: PlanScope, *, title: str, objective: str, steps: list[dict[str, Any]]
    ) -> dict[str, Any]: ...
    async def read(self, scope: PlanScope, *, plan_id: str) -> dict[str, Any]: ...
    async def list(self, scope: PlanScope) -> dict[str, Any]: ...
    async def update(
        self,
        scope: PlanScope,
        *,
        plan_id: str,
        expected_revision: int,
        title: str | None,
        objective: str | None,
        steps: list[dict[str, Any]] | None,
    ) -> dict[str, Any]: ...
    async def delete(
        self, scope: PlanScope, *, plan_id: str, expected_revision: int
    ) -> dict[str, Any]: ...


class MemoryPlanBackend:
    """Shared bounded native memory store; no files, global registry or scheduler.

    Retained until the last reference is dropped. Separate backend instances
    are independent. Defaults: 256 total plans, 64 per conversation/agent scope.
    """

    def __init__(self, *, max_plans: int = 256, max_plans_per_scope: int = 64) -> None:
        self._native = _MemoryPlanBackend(max_plans, max_plans_per_scope)

    async def _call(self, name: str, scope: PlanScope, arguments: dict[str, Any]) -> dict[str, Any]:
        if not isinstance(scope, PlanScope):
            raise InvalidInputError("a host-supplied PlanScope is required")
        token = _Cancellation()
        try:
            result = await self._native.call(
                name,
                (scope.room_id, scope.caller_id),
                json.dumps(arguments, allow_nan=False),
                token,
            )
            return json.loads(result)
        finally:
            token.cancel()

    async def create(
        self, scope: PlanScope, *, title: str, objective: str, steps: list[dict[str, Any]]
    ) -> dict[str, Any]:
        return await self._call(
            "plan_create", scope, {"title": title, "objective": objective, "steps": steps}
        )

    async def read(self, scope: PlanScope, *, plan_id: str) -> dict[str, Any]:
        return await self._call("plan_read", scope, {"plan_id": plan_id})

    async def list(self, scope: PlanScope) -> dict[str, Any]:
        return await self._call("plan_list", scope, {})

    async def update(
        self,
        scope: PlanScope,
        *,
        plan_id: str,
        expected_revision: int,
        title: str | None = None,
        objective: str | None = None,
        steps: list[dict[str, Any]] | None = None,
    ) -> dict[str, Any]:
        return await self._call(
            "plan_update",
            scope,
            {
                "plan_id": plan_id,
                "expected_revision": expected_revision,
                "title": title,
                "objective": objective,
                "steps": steps,
            },
        )

    async def delete(
        self, scope: PlanScope, *, plan_id: str, expected_revision: int
    ) -> dict[str, Any]:
        return await self._call(
            "plan_delete", scope, {"plan_id": plan_id, "expected_revision": expected_revision}
        )


class _PlanTool:
    name: ClassVar[str]
    method: ClassVar[str]

    def __init__(self, backend: PlanBackend) -> None:
        self._backend = backend

    @classmethod
    def contract(cls) -> dict[str, Any]:
        """Full framework-independent schemas derived from actual Rust types."""
        return json.loads(_PlanContract.definition_json(cls.name))

    @property
    def definition(self) -> dict[str, Any]:
        """Select this tool directly in RoomKit's tools list."""
        schema = self.contract()
        return {
            "name": schema["name"],
            "description": schema["description"],
            "parameters": schema["input_schema"],
        }

    async def call(self, arguments: dict[str, Any], *, scope: PlanScope) -> dict[str, Any]:
        """Standalone call with trusted owner and validated backend output."""
        if not isinstance(scope, PlanScope):
            raise InvalidInputError("a host-supplied PlanScope is required")
        inputs = json.loads(
            _PlanContract.input_json(self.name, json.dumps(arguments, allow_nan=False))
        )
        method = getattr(self._backend, self.method, None)
        if method is None:
            raise UnsupportedResourceError(f"backend does not support {self.name}")
        data = await method(scope, **inputs)
        return json.loads(_PlanContract.output_json(self.name, json.dumps(data, allow_nan=False)))

    async def handler(self, name: str, arguments: dict[str, Any]) -> str:
        """Use RoomKit's trusted conversation/channel identity for every call."""
        from roomkit.core.exceptions import ToolFailedError, ToolRefusedError, UnservedToolCallError
        from roomkit.tools.context import current_tool_call, current_tool_room_id

        if name != self.name:
            raise UnservedToolCallError(name)
        room, call = current_tool_room_id(), current_tool_call()
        actor = call.channel_id if call is not None else None
        if not room or not actor:
            raise ToolRefusedError("plan tools require a conversation and an acting agent")
        try:
            result = await self.call(arguments, scope=PlanScope(room, actor))
        except (InvalidInputError, AccessRefusedError) as error:
            raise ToolRefusedError(str(error)) from error
        except (LimitExceededError, UnsupportedResourceError) as error:
            raise ToolFailedError(str(error)) from error
        call.structured_content = result
        return json.dumps(result, ensure_ascii=False)


class PlanCreate(_PlanTool):
    """Create an owned plan and return its identifier and revision."""

    name = "plan_create"
    method = "create"


class PlanRead(_PlanTool):
    """Read the complete current plan."""

    name = "plan_read"
    method = "read"


class PlanList(_PlanTool):
    """List summaries in the trusted scope."""

    name = "plan_list"
    method = "list"


class PlanUpdate(_PlanTool):
    """Update selected fields with a mandatory expected revision."""

    name = "plan_update"
    method = "update"


class PlanDelete(_PlanTool):
    """Delete one version of a plan without cancelling any related execution."""

    name = "plan_delete"
    method = "delete"
