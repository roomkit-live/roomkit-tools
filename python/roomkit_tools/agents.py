"""Selectable delegation facades; orchestration belongs to the injected backend.

Importing this module needs no RoomKit. Inputs, outputs, bounds and schemas use
the Rust contracts. A Python backend never has to implement a Rust callback.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from typing import Any, ClassVar, Protocol

from ._native import (
    AccessRefusedError,
    InvalidInputError,
    LimitExceededError,
    UnsupportedResourceError,
    _AgentContract,
)


@dataclass(frozen=True)
class AgentScope:
    """Trusted host identity, supplied outside the model's arguments."""

    room_id: str
    caller_id: str

    def __post_init__(self) -> None:
        for value in (self.room_id, self.caller_id):
            if not isinstance(value, str) or not value.strip() or len(value.encode()) > 256:
                raise InvalidInputError("scope identifiers must contain 1..256 UTF-8 bytes")


class AgentBackend(Protocol):
    """An explicit, bounded task owner. Enforce scope in every method.

    Own cancellation/shutdown independently of a caller's lifetime. Document
    result delivery, retention and whether state survives host restarts.
    Returned dictionaries must satisfy the tool's Rust-derived output schema.
    """

    async def delegate(self, scope: AgentScope, *, profile: str, task: str) -> dict[str, Any]: ...

    async def status(self, scope: AgentScope, *, task_id: str | None) -> dict[str, Any]: ...

    async def cancel(self, scope: AgentScope, *, task_id: str) -> dict[str, Any]: ...

    async def followup(self, scope: AgentScope, *, agent_id: str, task: str) -> dict[str, Any]: ...

    async def send(self, scope: AgentScope, *, agent_id: str, message: str) -> dict[str, Any]: ...


class _AgentTool:
    name: ClassVar[str]
    method: ClassVar[str]

    def __init__(self, backend: AgentBackend) -> None:
        self._backend = backend

    @property
    def definition(self) -> dict[str, Any]:
        """RoomKit-compatible definition derived from the implementation types."""
        definition = self.contract()
        return {
            "name": definition["name"],
            "description": definition["description"],
            "parameters": definition["input_schema"],
        }

    @classmethod
    def contract(cls) -> dict[str, Any]:
        """Full framework-neutral Rust input/output schemas."""
        return json.loads(_AgentContract.definition_json(cls.name))

    async def call(self, arguments: dict[str, Any], *, scope: AgentScope) -> dict[str, Any]:
        """Standalone call with explicit trusted identity; no RoomKit import."""
        if not isinstance(scope, AgentScope):
            raise InvalidInputError("an AgentScope supplied by the host is required")
        inputs = json.loads(_AgentContract.input_json(self.name, json.dumps(arguments)))
        method = getattr(self._backend, self.method, None)
        if method is None:
            raise UnsupportedResourceError(f"backend does not support {self.name}")
        result = await method(scope, **inputs)
        return json.loads(_AgentContract.output_json(self.name, json.dumps(result)))

    async def handler(self, name: str, arguments: dict[str, Any]) -> str:
        """Resolve RoomKit's trusted caller; never accept scope from the model."""
        from roomkit.core.exceptions import ToolFailedError, ToolRefusedError, UnservedToolCallError
        from roomkit.tools.context import current_tool_call, current_tool_room_id

        if name != self.name:
            raise UnservedToolCallError(name)
        room, call = current_tool_room_id(), current_tool_call()
        actor = call.channel_id if call is not None else None
        if not room or not actor:
            raise ToolRefusedError("agent tools require a conversation and an acting agent")
        try:
            result = await self.call(arguments, scope=AgentScope(room, actor))
        except (InvalidInputError, AccessRefusedError) as error:
            raise ToolRefusedError(str(error)) from error
        except (LimitExceededError, UnsupportedResourceError) as error:
            raise ToolFailedError(str(error)) from error
        call.structured_content = result
        return json.dumps(result, ensure_ascii=False)


class DelegateTask(_AgentTool):
    """Start an isolated worker from a host-approved profile."""

    name = "delegate_task"
    method = "delegate"

    @property
    def definition(self) -> dict[str, Any]:
        definition = super().definition
        profiles = getattr(self._backend, "allowed_profiles", None)
        if profiles is not None:
            definition["parameters"]["properties"]["profile"]["enum"] = list(profiles)
        return definition


class TaskStatus(_AgentTool):
    """Inspect only this backend's tasks; does not start or cancel work."""

    name = "task_status"
    method = "status"


class CancelTask(_AgentTool):
    """Cancel an owned task; already ended tasks remain unchanged."""

    name = "cancel_task"
    method = "cancel"


class AgentFollowup(_AgentTool):
    """Start another mission in a retained idle agent's conversation."""

    name = "agent_followup"
    method = "followup"


class AgentSend(_AgentTool):
    """Queue a clarification for a running agent, without starting another task."""

    name = "agent_send"
    method = "send"
