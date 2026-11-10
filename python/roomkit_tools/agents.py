"""Selectable delegation facades; orchestration belongs to the injected backend.

`AskUserTool` asks the person questions through the same backend, which shows
them and returns the answers.

Importing this module needs no RoomKit. Inputs, outputs, bounds and schemas use
the Rust contracts. A Python backend never has to implement a Rust callback.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from typing import Any, ClassVar, Protocol

from ._native import (
    InvalidInputError,
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

    async def ask(self, scope: AgentScope, *, questions: list[dict[str, Any]]) -> dict[str, Any]:
        """Show the person the questions and return their answers, one per
        question; raise AccessRefusedError with the reason when nobody can
        answer."""
        ...


def _as_rust_reads(arguments: dict[str, Any]) -> str:
    """The arguments as Rust's ``read`` serializes them, compact and unescaped,
    so that the 1 MiB bound counts the same bytes. Text that is not Unicode,
    such as a lone surrogate, cannot reach Rust: refuse it as Rust would."""
    text = json.dumps(arguments, ensure_ascii=False, separators=(",", ":"))
    try:
        text.encode("utf-8")
    except UnicodeEncodeError:
        raise InvalidInputError("arguments must be Unicode text") from None
    return text


class _AgentTool:
    name: ClassVar[str]
    method: ClassVar[str]

    def __init__(self, backend: AgentBackend) -> None:
        self._backend = backend

    @classmethod
    def definition(cls) -> dict[str, Any]:
        """Full framework-neutral Rust input/output schemas."""
        return json.loads(_AgentContract.definition_json(cls.name))

    async def call(self, arguments: dict[str, Any], *, scope: AgentScope) -> dict[str, Any]:
        """Standalone call with explicit trusted identity; no RoomKit import."""
        if not isinstance(scope, AgentScope):
            raise InvalidInputError("an AgentScope supplied by the host is required")
        inputs = json.loads(_AgentContract.input_json(self.name, _as_rust_reads(arguments)))
        method = getattr(self._backend, self.method, None)
        if method is None:
            raise UnsupportedResourceError(f"backend does not support {self.name}")
        result = await method(scope, **inputs)
        return json.loads(_AgentContract.output_json(self.name, json.dumps(result)))


class DelegateTaskTool(_AgentTool):
    """Start an isolated worker from a host-approved profile."""

    name = "delegate_task"
    method = "delegate"


class TaskStatusTool(_AgentTool):
    """Inspect only this backend's tasks; does not start or cancel work."""

    name = "task_status"
    method = "status"


class CancelTaskTool(_AgentTool):
    """Cancel an owned task; already ended tasks remain unchanged."""

    name = "cancel_task"
    method = "cancel"


class AgentFollowupTool(_AgentTool):
    """Start another mission in a retained idle agent's conversation."""

    name = "agent_followup"
    method = "followup"


class AgentSendTool(_AgentTool):
    """Queue a clarification for a running agent, without starting another task."""

    name = "agent_send"
    method = "send"


class AskUserTool(_AgentTool):
    """Ask the person one to four questions and wait for their answers."""

    name = "ask_user"
    method = "ask"

    @classmethod
    def read(cls, arguments: dict[str, Any]) -> dict[str, Any]:
        """The questions of an ``ask_user`` call's arguments, checked as
        :meth:`call` checks them, for a host that serves the call itself.

        Raises InvalidInputError or LimitExceededError saying what the model
        should fix.
        """
        return json.loads(_AgentContract.input_json(cls.name, _as_rust_reads(arguments)))
