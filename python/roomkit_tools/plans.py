"""Selectable plan CRUD with Rust contracts and explicitly injected storage."""

from __future__ import annotations

import json
from dataclasses import dataclass
from typing import Any, ClassVar, Protocol

from ._native import (
    InvalidInputError,
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
    ``PlanUpdateTool`` hands ``update`` the complete ``steps``, its
    ``step_changes`` already applied; only a direct caller passes them.
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
        step_changes: list[dict[str, Any]] | None = None,
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
        step_changes: list[dict[str, Any]] | None = None,
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
                "step_changes": step_changes,
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
    def definition(cls) -> dict[str, Any]:
        """Full framework-independent schemas derived from actual Rust types."""
        return json.loads(_PlanContract.definition_json(cls.name))

    async def call(self, arguments: dict[str, Any], *, scope: PlanScope) -> dict[str, Any]:
        """Standalone call with trusted owner and validated backend output."""
        if not isinstance(scope, PlanScope):
            raise InvalidInputError("a host-supplied PlanScope is required")
        given = _PlanContract.input_json(self.name, json.dumps(arguments, allow_nan=False))
        data = await self._run(scope, given)
        return json.loads(self._output(given, json.dumps(data, allow_nan=False)))

    async def _run(self, scope: PlanScope, given: str) -> dict[str, Any]:
        """The backend's result for the validated arguments ``given``."""
        return await self._method(self.method)(scope, **json.loads(given))

    def _output(self, given: str, data: str) -> str:
        """The validated envelope of the backend's result."""
        return _PlanContract.output_json(self.name, data)

    def _method(self, name: str) -> Any:
        method = getattr(self._backend, name, None)
        if method is None:
            raise UnsupportedResourceError(f"backend does not support {self.name}")
        return method


class PlanCreateTool(_PlanTool):
    """Create an owned plan and return its identifier and revision."""

    name = "plan_create"
    method = "create"


class PlanReadTool(_PlanTool):
    """Read the complete current plan."""

    name = "plan_read"
    method = "read"


class PlanListTool(_PlanTool):
    """List summaries in the trusted scope."""

    name = "plan_list"
    method = "list"


class PlanUpdateTool(_PlanTool):
    """Update selected fields with a mandatory expected revision.

    ``step_changes`` are applied here, to the plan the backend reads, and the
    plan they make is checked: the backend's ``update`` receives the complete
    ``steps``. The result's text names only the changed steps, or shows the
    whole plan when the title or objective change too.
    """

    name = "plan_update"
    method = "update"

    async def _run(self, scope: PlanScope, given: str) -> dict[str, Any]:
        inputs = json.loads(given)
        if "step_changes" in inputs:
            plan = await self._method("read")(scope, plan_id=inputs["plan_id"])
            resolved = _PlanContract.resolve_update_json(json.dumps(plan, allow_nan=False), given)
            inputs = json.loads(resolved)
        return await self._method(self.method)(scope, **inputs)

    def _output(self, given: str, data: str) -> str:
        return _PlanContract.update_output_json(given, data)


class PlanDeleteTool(_PlanTool):
    """Delete one version of a plan without cancelling any related execution."""

    name = "plan_delete"
    method = "delete"
