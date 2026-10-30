"""Optional RoomKit backend for independently selectable delegation tools.

One backend belongs to one conversation/principal and one asyncio loop. Static
profiles and dynamic resolvers return blueprints, not live shared channels.
Each delegation owns a fresh provider, channel and child room. Follow-ups retain
that instance until idle expiry or close. Close the backend before its RoomKit.
State is in memory; child-room history stays in the RoomKit store.
This is orchestration, not a sandbox or durable task service.
"""

from __future__ import annotations

import asyncio
import json
import math
from collections.abc import Awaitable, Callable, Mapping
from dataclasses import dataclass, field
from types import MappingProxyType
from typing import TYPE_CHECKING, Any
from uuid import uuid4

from ._native import AccessRefusedError, InvalidInputError, LimitExceededError, _AgentContract
from .agents import AgentScope

if TYPE_CHECKING:
    from roomkit import RoomKit
    from roomkit.providers.ai.base import AIProvider


@dataclass(frozen=True)
class AgentConfig:
    """Host-owned blueprint. Factories must return fresh, backend-owned objects.

    Tools and providers are selected by the host, never by model arguments.
    The provider factory is synchronous; asynchronous configuration belongs in
    the resolver. Closing an instance closes its provider through AIChannel.
    Tools with their own lifecycle remain the host's responsibility.
    """

    provider_factory: Callable[[], AIProvider]
    system_prompt: str
    tools_factory: Callable[[], list[Any]] = list
    max_tool_rounds: int = 12
    tool_timeout_seconds: float = 30
    tool_loop_timeout_seconds: float = 180

    def __post_init__(self) -> None:
        if not callable(self.provider_factory) or not callable(self.tools_factory):
            raise InvalidInputError("agent factories must be callable")
        if not isinstance(self.system_prompt, str) or len(self.system_prompt.encode()) > 65_536:
            raise InvalidInputError("system_prompt must be text of at most 65536 bytes")
        if type(self.max_tool_rounds) is not int or not 1 <= self.max_tool_rounds <= 100:
            raise InvalidInputError("max_tool_rounds must be in 1..100")
        for value in (self.tool_timeout_seconds, self.tool_loop_timeout_seconds):
            if isinstance(value, bool) or not isinstance(value, (int, float)):
                raise InvalidInputError("agent deadlines must be finite numbers")
            if not math.isfinite(value) or not 0 < value <= 3600:
                raise InvalidInputError("agent deadlines must be in (0, 3600] seconds")


@dataclass(frozen=True)
class AgentRequest:
    """Immutable dynamic-resolution input, including host-supplied identity."""

    profile: str
    task: str
    scope: AgentScope


@dataclass(eq=False)
class _Session:
    agent_id: str
    profile: str
    channel: Any = None
    child_room_id: str | None = None
    current: _Record | None = None
    closing: bool = False
    cleanup: asyncio.Task | None = None
    idle: asyncio.Task | None = None
    messages: int = 0
    messages_inflight: int = 0

    @property
    def available(self) -> bool:
        return self.channel is not None and not self.closing


@dataclass(eq=False)
class _Record:
    session: _Session
    task: str
    task_id: str | None = None
    submitted: bool = False
    followup: bool = False
    status: str = "running"
    delivery: str = "pending"
    result: str | None = None
    result_truncated: bool = False
    suppress: bool = False
    done: asyncio.Event = field(default_factory=asyncio.Event)

    def info(self) -> dict[str, Any]:
        return {
            "task_id": self.task_id,
            "agent_id": self.session.agent_id,
            "profile": self.session.profile,
            "status": self.status,
            "delivery": self.delivery,
            "result": self.result,
            "result_truncated": self.result_truncated,
            "agent_available": self.session.available,
        }


class RoomKitAgentBackend:
    """One conversation/principal, one asyncio loop and bounded retained sessions.

    Choose static profiles or a resolver plus an allow list. Configuration is
    resolved once per instance; follow-ups retain it. Each mission gets its own
    task ID. Idle sessions expire after idle_timeout (default 300 seconds).
    All receipts remain until close; capacity is never silently recycled.

    Caller cancellation does not revoke an accepted launch or message. Recover
    with status rather than blindly retrying. Cancellation/close own cleanup.
    Always use async with inside the owning RoomKit's lifetime.
    """

    def __init__(
        self,
        kit: RoomKit,
        *,
        scope: AgentScope,
        profiles: Mapping[str, AgentConfig] | None = None,
        resolver: Callable[[AgentRequest], Awaitable[AgentConfig]] | None = None,
        allowed_profiles: tuple[str, ...] | None = None,
        max_concurrent: int = 2,
        max_tasks: int = 8,
        resolver_timeout: float = 10,
        idle_timeout: float = 300,
        max_messages: int = 32,
    ) -> None:
        if not isinstance(scope, AgentScope):
            raise InvalidInputError("scope must be supplied by the host")
        if (profiles is None) == (resolver is None):
            raise InvalidInputError("choose either profiles or a resolver")
        if profiles is not None and allowed_profiles is not None:
            raise InvalidInputError("static profile names come from profiles")
        self._profiles = MappingProxyType(dict(profiles)) if profiles is not None else None
        names = (
            tuple(self._profiles) if self._profiles is not None else tuple(allowed_profiles or ())
        )
        if not 1 <= len(names) <= 64:
            raise InvalidInputError("supply 1..64 allowed profiles")
        for name in names:
            if not isinstance(name, str) or not name.strip() or len(name.encode()) > 128:
                raise InvalidInputError("profile names must contain 1..128 bytes")
        if len(set(names)) != len(names):
            raise InvalidInputError("profile names must be distinct")
        if self._profiles is not None and any(
            not isinstance(c, AgentConfig) for c in self._profiles.values()
        ):
            raise InvalidInputError("profiles must contain AgentConfig blueprints")
        if resolver is not None and not callable(resolver):
            raise InvalidInputError("resolver must be callable")
        if (
            type(max_tasks) is not int
            or type(max_concurrent) is not int
            or not 1 <= max_concurrent <= max_tasks <= 64
        ):
            raise InvalidInputError("require 1 <= max_concurrent <= max_tasks <= 64")
        if type(max_messages) is not int or not 1 <= max_messages <= 128:
            raise InvalidInputError("max_messages must be in 1..128 per instance")
        for value, name, maximum in (
            (resolver_timeout, "resolver_timeout", 60),
            (idle_timeout, "idle_timeout", 3600),
        ):
            if (
                isinstance(value, bool)
                or not isinstance(value, (int, float))
                or not math.isfinite(value)
                or not 0 < value <= maximum
            ):
                raise InvalidInputError(f"{name} must be in (0, {maximum}] seconds")
        self.kit, self.scope = kit, scope
        self.allowed_profiles = names
        self._resolver = resolver
        self._max_concurrent, self._max_tasks = max_concurrent, max_tasks
        self._resolver_timeout, self._idle_timeout = resolver_timeout, idle_timeout
        self._max_messages = max_messages
        self._sessions: dict[str, _Session] = {}
        self._tasks: dict[str, _Record] = {}
        self._pending: list[_Record] = []
        self._launches: set[asyncio.Task] = set()
        self._messages: set[asyncio.Task] = set()
        self._cancellations: dict[str, asyncio.Task] = {}
        self._closing = False
        self._close_task: asyncio.Task | None = None
        self._loop: asyncio.AbstractEventLoop | None = None

    def _check(self, scope: AgentScope) -> None:
        if scope != self.scope:
            raise AccessRefusedError("this backend belongs to another conversation or principal")
        loop = asyncio.get_running_loop()
        if self._loop is None:
            self._loop = loop
        elif self._loop is not loop:
            raise AccessRefusedError("agent backend cannot move between event loops")

    def _admit(self) -> None:
        if self._closing:
            raise AccessRefusedError("agent backend is closed")
        if len(self._tasks) + len(self._pending) >= self._max_tasks:
            raise LimitExceededError("retained task capacity reached")
        if sum(s.current is not None for s in self._sessions.values()) >= self._max_concurrent:
            raise LimitExceededError("concurrent agent capacity reached")

    @staticmethod
    def _validate(name: str, arguments: dict[str, Any]) -> None:
        _AgentContract.input_json(name, json.dumps(arguments))

    @staticmethod
    def _own(coroutine, operations):
        operation = asyncio.create_task(coroutine)
        operations.add(operation)

        def finished(future):
            operations.discard(future)
            if not future.cancelled():
                future.exception()

        operation.add_done_callback(finished)
        return operation

    def _remember(self, record: _Record, task_id: str, room_id: str) -> None:
        record.task_id = task_id
        record.session.child_room_id = room_id
        if record in self._pending:
            self._pending.remove(record)
        self._tasks[task_id] = record

    async def delegate(self, scope: AgentScope, *, profile: str, task: str) -> dict[str, Any]:
        """Resolve a profile once and create a fresh reusable instance."""
        self._check(scope)
        self._validate("delegate_task", {"profile": profile, "task": task})
        if profile not in self.allowed_profiles:
            raise AccessRefusedError("unknown or disallowed profile: " + profile)
        self._admit()
        session = _Session("agent-" + uuid4().hex, profile)
        record = _Record(session, task)
        session.current = record
        self._sessions[session.agent_id] = session
        self._pending.append(record)
        return await asyncio.shield(self._own(self._launch(record), self._launches))

    async def _launch(self, record: _Record) -> dict[str, Any]:
        from roomkit import AIChannel
        from roomkit.tasks.handback import CALLER_HANDS_BACK

        session = record.session
        try:
            if self._profiles is not None:
                config = self._profiles[session.profile]
            else:
                async with asyncio.timeout(self._resolver_timeout):
                    config = await self._resolver(
                        AgentRequest(session.profile, record.task, self.scope)
                    )
            if not isinstance(config, AgentConfig):
                raise InvalidInputError("resolver must return an AgentConfig")
            if self._closing:
                raise AccessRefusedError("agent backend is closing")
            tools = list(config.tools_factory())
            provider = config.provider_factory()
            try:
                session.channel = AIChannel(
                    session.agent_id,
                    provider=provider,
                    system_prompt=config.system_prompt,
                    tools=tools,
                    max_tool_rounds=config.max_tool_rounds,
                    tool_timeout_seconds=config.tool_timeout_seconds,
                    tool_loop_timeout_seconds=config.tool_loop_timeout_seconds,
                )
            except BaseException:
                await provider.close()
                raise
            self.kit.register_channel(session.channel)
            handle = await self.kit.delegate(
                self.scope.room_id,
                session.agent_id,
                f"[{session.profile}] {record.task}",
                brief=record.task,
                notify=CALLER_HANDS_BACK,
                share_channels=[],
                conversation=0,
                on_complete=lambda result: self._complete(record, result),
            )
            record.submitted = True
            self._remember(record, handle.id, handle.child_room_id)
            return record.info()
        except BaseException:
            if record in self._pending:
                self._pending.remove(record)
            await self._cleanup(session)
            if record.task_id is None:
                self._sessions.pop(session.agent_id, None)
            raise

    def _session(self, agent_id: str) -> _Session:
        if agent_id not in self._sessions:
            raise AccessRefusedError("unknown agent in this backend")
        return self._sessions[agent_id]

    async def followup(self, scope: AgentScope, *, agent_id: str, task: str) -> dict[str, Any]:
        """Run a new mission in the retained child room, with frozen configuration."""
        self._check(scope)
        self._validate("agent_followup", {"agent_id": agent_id, "task": task})
        session = self._session(agent_id)
        self._admit()
        if not session.available:
            raise AccessRefusedError("agent session expired or closed; delegate a new instance")
        if session.current is not None or session.messages_inflight:
            raise AccessRefusedError("agent is busy; wait for completion or use agent_send")
        if session.idle is not None:
            session.idle.cancel()
        record = _Record(session, task, task_id="task-" + uuid4().hex, followup=True)
        session.current = record
        self._tasks[record.task_id] = record
        return await asyncio.shield(self._own(self._submit_followup(record), self._launches))

    async def _submit_followup(self, record: _Record) -> dict[str, Any]:
        from roomkit.tasks import DelegatedTask

        session = record.session
        handle = DelegatedTask(
            id=record.task_id,
            child_room_id=session.child_room_id,
            parent_room_id=self.scope.room_id,
            agent_id=session.agent_id,
            task=f"[{session.profile}] {record.task}",
            brief=record.task,
        )
        try:
            await self.kit.task_runner.submit(
                self.kit, handle, on_complete=lambda result: self._complete(record, result)
            )
            record.submitted = True
            return record.info()
        except BaseException:
            record.status = "failed"
            record.result = "follow-up submission failed"
            record.delivery = "suppressed"
            self._settle(record)
            raise

    async def _complete(self, record, result):
        from roomkit.tasks.handback import bounded, hand_back, result_text

        self._remember(record, result.task_id, result.child_room_id)
        record.status = str(result.status)
        raw = (result.output or result.error or "").encode()
        record.result = raw[:8192].decode(errors="ignore")
        record.result_truncated = len(raw) > 8192
        # The worker has ended. A principal receiving its report can immediately
        # request another mission, even while this relay is still being delivered.
        if record.session.current is record:
            record.session.current = None
        self._arm_idle(record.session)
        try:
            if self._closing or (record.suppress and record.status == "cancelled"):
                record.delivery = "suppressed"
                return
            header = (
                f"[Background task from {record.session.profile} {record.status}. "
                f"task_id={record.task_id}, agent_id={record.session.agent_id}]\n"
                f"Task: {record.task}"
            )
            outcome = await hand_back(
                self.kit,
                self.scope.room_id,
                self.scope.caller_id,
                result_text(header, bounded(result.output or result.error or "")),
                0,
                metadata={
                    "task_id": record.task_id,
                    "agent_id": record.session.agent_id,
                    "child_room_id": result.child_room_id,
                    "task_status": record.status,
                },
            )
            record.delivery = (
                "delivered"
                if outcome is not None and outcome.status == "sent" and outcome.turn_complete
                else "failed"
            )
        except BaseException:
            record.delivery = "failed"
            raise
        finally:
            self._settle(record)

    def _settle(self, record: _Record) -> None:
        record.done.set()
        if record.session.current is record:
            record.session.current = None
        self._arm_idle(record.session)

    def _arm_idle(self, session: _Session) -> None:
        if self._closing or not session.available or session.current or session.messages_inflight:
            return
        if session.idle is not None:
            session.idle.cancel()
        session.idle = asyncio.create_task(self._expire(session))
        session.idle.add_done_callback(lambda f: f.exception() if not f.cancelled() else None)

    async def _expire(self, session: _Session) -> None:
        await asyncio.sleep(self._idle_timeout)
        if session.current is None and not session.messages_inflight:
            await self._cleanup(session)

    async def send(self, scope: AgentScope, *, agent_id: str, message: str) -> dict[str, Any]:
        """Queue to the active loop and record in history without waking a new turn."""
        self._check(scope)
        self._validate("agent_send", {"agent_id": agent_id, "message": message})
        if self._closing:
            raise AccessRefusedError("agent backend is closed")
        session = self._session(agent_id)
        record = session.current
        receipt = {
            "agent_id": agent_id,
            "task_id": record.task_id if record else None,
            "message_id": None,
            "status": "not_running",
            "history_recorded": False,
        }
        if not session.available or record is None or record.status != "running":
            return receipt
        if session.messages + session.messages_inflight >= self._max_messages:
            raise LimitExceededError("agent message capacity reached")
        session.messages_inflight += 1
        return await asyncio.shield(
            self._own(self._send(session, record, message, receipt), self._messages)
        )

    async def _send(self, session, record, message, receipt):
        from roomkit.models.steering import InjectMessage

        try:
            if session.current is not record or record.status != "running" or not session.available:
                return receipt
            message_id = "message-" + uuid4().hex
            content = f"[Clarification {message_id} from the principal]\n{message}"
            reached = await self.kit.steer(
                InjectMessage(content=content), session.agent_id, room_id=session.child_room_id
            )
            if reached == 0:
                return receipt
            session.messages += 1
            receipt.update(message_id=message_id, status="queued")
            try:
                saved = await self.kit.post(
                    session.child_room_id,
                    content,
                    addressed_to=[],
                    metadata={"agent_message_id": message_id, "task_id": record.task_id},
                    idempotency_key=message_id,
                )
                receipt["history_recorded"] = saved.status == "delivered"
            except Exception:
                # Queuing already happened: do not return an ambiguous error
                # that would encourage a duplicate send on retry.
                receipt["history_recorded"] = False
            return receipt
        finally:
            session.messages_inflight -= 1
            self._arm_idle(session)

    def _record(self, task_id: str) -> _Record:
        if task_id not in self._tasks:
            raise AccessRefusedError("unknown task in this backend")
        return self._tasks[task_id]

    async def status(self, scope: AgentScope, *, task_id: str | None = None) -> dict[str, Any]:
        """Retained observed receipts; no model calls or implicit execution."""
        self._check(scope)
        records = [self._record(task_id)] if task_id is not None else self._tasks.values()
        return {
            "tasks": [record.info() for record in records],
            "pending_launches": len(self._pending),
        }

    async def cancel(self, scope: AgentScope, *, task_id: str) -> dict[str, Any]:
        """Cancel exactly this mission; its instance can be continued afterwards."""
        self._check(scope)
        record = self._record(task_id)
        if record.status != "running":
            return {"task": record.info(), "cancelled": False}
        if task_id not in self._cancellations:
            record.suppress = True
            operation = asyncio.create_task(self._cancel(record))
            operation.add_done_callback(lambda f: f.exception() if not f.cancelled() else None)
            self._cancellations[task_id] = operation
        return await asyncio.shield(self._cancellations[task_id])

    async def _cancel(self, record: _Record) -> dict[str, Any]:
        cancel = self.kit.task_runner.cancel if record.followup else self.kit.cancel_task
        cancelled = await cancel(record.task_id)
        return {"task": record.info(), "cancelled": cancelled}

    async def wait(self, task_id: str) -> dict[str, Any]:
        """Host wait for one mission and its relay; caller timeout does not cancel work."""
        self._check(self.scope)
        record = self._record(task_id)
        await record.done.wait()
        return record.info()

    async def _cleanup(self, session: _Session) -> None:
        if session.cleanup is None:
            session.closing = True
            session.cleanup = asyncio.create_task(self._release(session))
        await asyncio.shield(session.cleanup)

    async def _release(self, session: _Session) -> None:
        try:
            if session.child_room_id is not None:
                await self.kit.detach_channel(session.child_room_id, session.agent_id)
                await self.kit.close_room(session.child_room_id)
        finally:
            if session.channel is not None:
                self.kit.unregister_channel(session.agent_id)
                await session.channel.close()
                session.channel = None

    async def aclose(self) -> None:
        """Permanently stop owned work and release sessions even if this waiter is cancelled."""
        self._check(self.scope)
        if self._close_task is None:
            self._closing = True
            self._close_task = asyncio.create_task(self._close())
        await asyncio.shield(self._close_task)

    async def _close(self) -> None:
        for launch in tuple(self._launches):
            launch.cancel()
        await asyncio.gather(*self._launches, return_exceptions=True)

        # A coroutine cancelled before its first step cannot execute its own
        # cleanup. Settle reservations that were never accepted by the runner.
        for record in (*self._pending, *self._tasks.values()):
            if not record.submitted and not record.done.is_set():
                record.status, record.delivery = "cancelled", "suppressed"
                self._settle(record)
        self._pending.clear()

        async def finish(session):
            if session.idle is not None:
                session.idle.cancel()
                await asyncio.gather(session.idle, return_exceptions=True)
            try:
                record = session.current
                if record is not None and record.submitted and not record.done.is_set():
                    await self._cancel(record)
            finally:
                await self._cleanup(session)

        outcomes = await asyncio.gather(
            *(finish(s) for s in tuple(self._sessions.values())),
            *self._cancellations.values(),
            return_exceptions=True,
        )
        # Also join completed missions whose report is still being delivered.
        # RoomKit owns terminal callbacks and waits for them on cancellation.
        tails = await asyncio.gather(
            *(self._cancel(r) for r in self._tasks.values() if r.submitted and not r.done.is_set()),
            *self._messages,
            return_exceptions=True,
        )
        failures = [r for r in (*outcomes, *tails) if isinstance(r, BaseException)]
        if failures:
            raise BaseExceptionGroup("agent backend cleanup failed", failures)

    async def __aenter__(self) -> RoomKitAgentBackend:
        self._check(self.scope)
        if self._closing:
            raise AccessRefusedError("agent backend is closed")
        return self

    async def __aexit__(self, *args: Any) -> None:
        await self.aclose()
