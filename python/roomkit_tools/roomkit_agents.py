"""Optional RoomKit backend for independently selectable delegation tools.

One backend belongs to one conversation/principal and one asyncio loop. Static
profiles and dynamic resolvers return blueprints, not live shared channels.
Each successful launch owns a fresh provider, channel and child room. Close the
backend before its RoomKit. State is in memory; child-room history stays in the
RoomKit store. This is orchestration, not a sandbox or durable task service.
"""

from __future__ import annotations

import asyncio
import math
from collections.abc import Awaitable, Callable, Mapping
from dataclasses import dataclass, field
from types import MappingProxyType
from typing import TYPE_CHECKING, Any
from uuid import uuid4

from ._native import AccessRefusedError, InvalidInputError, LimitExceededError
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


@dataclass
class _Record:
    request: AgentRequest
    agent_id: str
    task_id: str | None = None
    child_room_id: str | None = None
    channel: Any = None
    submitted: bool = False
    status: str = "running"
    delivery: str = "pending"
    result: str | None = None
    result_truncated: bool = False
    cleanup: asyncio.Task | None = None
    done: asyncio.Event = field(default_factory=asyncio.Event)

    def info(self) -> dict[str, Any]:
        return {
            "task_id": self.task_id,
            "agent_id": self.agent_id,
            "profile": self.request.profile,
            "status": self.status,
            "delivery": self.delivery,
            "result": self.result,
            "result_truncated": self.result_truncated,
        }


class RoomKitAgentBackend:
    """Scoped task registry using RoomKit's existing delegate/relay lifecycle.

    Supply either ``profiles`` or ``resolver`` plus ``allowed_profiles``. A
    resolver is called once per launch, under a deadline, after scope/profile
    and capacity checks. Its returned frozen blueprint applies to that instance.

    Cancelling a delegate caller does not revoke an accepted launch: inspect
    status before retrying. cancel/close own shielded cleanup tasks. Explicit
    ``async with`` (inside RoomKit's lifetime) is required. Total launch capacity
    includes retained ended tasks; construct a new backend to begin a new scope.
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
        if not 1 <= len(names) <= 64 or len(set(names)) != len(names):
            raise InvalidInputError("supply 1..64 distinct allowed profiles")
        for name in names:
            if not isinstance(name, str) or not name.strip() or len(name.encode()) > 128:
                raise InvalidInputError("profile names must contain 1..128 bytes")
        if self._profiles is not None and any(
            not isinstance(config, AgentConfig) for config in self._profiles.values()
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
        if (
            isinstance(resolver_timeout, bool)
            or not isinstance(resolver_timeout, (int, float))
            or not math.isfinite(resolver_timeout)
            or not 0 < resolver_timeout <= 60
        ):
            raise InvalidInputError("resolver_timeout must be in (0, 60] seconds")
        self.kit, self.scope = kit, scope
        self.allowed_profiles = names
        self._resolver = resolver
        self._max_concurrent, self._max_tasks = max_concurrent, max_tasks
        self._resolver_timeout = resolver_timeout
        self._records: dict[str, _Record] = {}
        self._tasks: dict[str, _Record] = {}
        self._launches: set[asyncio.Task] = set()
        self._cancellations: dict[str, asyncio.Task] = {}
        self._suppress: set[str] = set()
        self._closing = False
        self._close_task: asyncio.Task | None = None
        self._loop: asyncio.AbstractEventLoop | None = None
        self._hook_names: list[str] = []
        self._install_hooks()

    def _check(self, scope: AgentScope) -> None:
        if scope != self.scope:
            raise AccessRefusedError("this backend belongs to another conversation or principal")
        loop = asyncio.get_running_loop()
        if self._loop is None:
            self._loop = loop
        elif self._loop is not loop:
            raise AccessRefusedError("agent backend cannot move between event loops")

    def _install_hooks(self) -> None:
        from roomkit import HookExecution, HookResult, HookTrigger

        prefix = "roomkit-tools-agents-" + uuid4().hex

        def register(trigger, callback, *, execution=HookExecution.ASYNC):
            name = prefix + "-" + str(trigger)
            self._hook_names.append(name)
            self.kit.hook(trigger, name=name, execution=execution)(callback)

        def owned(event):
            if event.room_id != self.scope.room_id:
                return None
            return self._records.get(event.metadata.get("agent_id")) or self._tasks.get(
                event.metadata.get("task_id")
            )

        async def started(event, _context):
            record = owned(event)
            if record is not None:
                record.task_id = event.metadata["task_id"]
                record.child_room_id = event.metadata["child_room_id"]
                self._tasks[record.task_id] = record

        async def completed(event, _context):
            record = owned(event)
            if record is not None:
                record.status = str(event.metadata["task_status"])
                raw = event.content.body.encode()
                record.result = raw[:8192].decode(errors="ignore")
                record.result_truncated = len(raw) > 8192

        async def before_delivery(event, _context):
            record = owned(event)
            if record is not None and (
                self._closing or (record.status == "cancelled" and record.task_id in self._suppress)
            ):
                record.delivery = "suppressed"
                return HookResult.block("task notification suppressed by its owner")
            return HookResult.allow()

        async def delivered(event, _context):
            record = owned(event)
            if record is not None and record.delivery != "suppressed":
                outcome = event.metadata.get("delivery_outcome", {})
                record.delivery = (
                    "delivered"
                    if outcome.get("status") == "sent" and outcome.get("turn_complete")
                    else "failed"
                )

        register(HookTrigger.ON_TASK_DELEGATED, started)
        register(HookTrigger.ON_TASK_COMPLETED, completed)
        register(HookTrigger.BEFORE_DELIVER, before_delivery, execution=HookExecution.SYNC)
        register(HookTrigger.AFTER_DELIVER, delivered)

    async def delegate(self, scope: AgentScope, *, profile: str, task: str) -> dict[str, Any]:
        """Resolve once and launch; caller cancellation leaves the owned launch alive."""
        self._check(scope)
        # Also validate direct host calls, not only model tool calls.
        import json

        from ._native import _AgentContract

        _AgentContract.input_json("delegate_task", json.dumps({"profile": profile, "task": task}))
        if self._closing:
            raise AccessRefusedError("agent backend is closed")
        if profile not in self.allowed_profiles:
            raise AccessRefusedError("unknown or disallowed profile: " + profile)
        if len(self._records) >= self._max_tasks:
            raise LimitExceededError("retained task capacity reached")
        if (
            sum(not record.done.is_set() for record in self._records.values())
            >= self._max_concurrent
        ):
            raise LimitExceededError("concurrent agent capacity reached")
        record = _Record(AgentRequest(profile, task, scope), "agent-" + uuid4().hex)
        self._records[record.agent_id] = record  # Reserve before any await.
        operation = asyncio.create_task(
            self._launch(record), name="agent-launch:" + record.agent_id
        )
        self._launches.add(operation)
        operation.add_done_callback(self._launch_done)
        return await asyncio.shield(operation)

    def _launch_done(self, operation: asyncio.Task) -> None:
        self._launches.discard(operation)
        if not operation.cancelled():
            operation.exception()  # Retrieve errors even if the caller was cancelled.

    async def _launch(self, record: _Record) -> dict[str, Any]:
        from roomkit import AIChannel

        try:
            if self._profiles is not None:
                config = self._profiles[record.request.profile]
            else:
                async with asyncio.timeout(self._resolver_timeout):
                    config = await self._resolver(record.request)
            if not isinstance(config, AgentConfig):
                raise InvalidInputError("resolver must return an AgentConfig")
            if self._closing:
                raise AccessRefusedError("agent backend is closing")
            # Resolve tools before acquiring the provider, so tool-factory failure
            # cannot strand a provider. The provider is owned from this point.
            selected = list(config.tools_factory())
            provider = config.provider_factory()
            try:
                record.channel = AIChannel(
                    record.agent_id,
                    provider=provider,
                    system_prompt=config.system_prompt,
                    tools=selected,
                    max_tool_rounds=config.max_tool_rounds,
                    tool_timeout_seconds=config.tool_timeout_seconds,
                    tool_loop_timeout_seconds=config.tool_loop_timeout_seconds,
                )
            except BaseException:
                await provider.close()
                raise
            self.kit.register_channel(record.channel)

            async def finished(result):
                # Called after RoomKit's relay attempt. Cleanup must survive a
                # cancelled parent await and runs outside the worker's own task.
                record.task_id, record.child_room_id = result.task_id, result.child_room_id
                self._tasks[result.task_id] = record
                await self._cleanup(record)

            handle = await self.kit.delegate(
                self.scope.room_id,
                record.agent_id,
                f"[{record.request.profile}] {record.request.task}",
                brief=record.request.task,
                notify=self.scope.caller_id,
                hand_back="relay",
                share_channels=[],
                conversation=0,
                on_complete=finished,
            )
            record.submitted = True
            record.task_id = handle.id
            record.child_room_id = handle.child_room_id
            self._tasks[record.task_id] = record
            return record.info()
        except BaseException:
            await self._cleanup(record)
            # No accepted receipt: failure does not consume launch capacity.
            if record.task_id is None:
                self._records.pop(record.agent_id, None)
            raise

    async def _cleanup(self, record: _Record) -> None:
        if record.cleanup is None:
            record.cleanup = asyncio.create_task(self._release(record))
        await asyncio.shield(record.cleanup)

    async def _release(self, record: _Record) -> None:
        try:
            if record.child_room_id is not None:
                await self.kit.detach_channel(record.child_room_id, record.agent_id)
                await self.kit.close_room(record.child_room_id)
        finally:
            try:
                if record.channel is not None:
                    self.kit.unregister_channel(record.agent_id)
                    await record.channel.close()
                    record.channel = None
            finally:
                record.done.set()

    def _record(self, task_id: str) -> _Record:
        if task_id not in self._tasks:
            raise AccessRefusedError("unknown task in this backend")
        return self._tasks[task_id]

    async def status(self, scope: AgentScope, *, task_id: str | None = None) -> dict[str, Any]:
        """Retained observed states; no provider calls and no implicit refresh."""
        self._check(scope)
        records = [self._record(task_id)] if task_id is not None else self._tasks.values()
        return {
            "tasks": [record.info() for record in records],
            "pending_launches": sum(record.task_id is None for record in self._records.values()),
        }

    async def cancel(self, scope: AgentScope, *, task_id: str) -> dict[str, Any]:
        """Stop work through RoomKit; return the state without a duplicate relay."""
        self._check(scope)
        record = self._record(task_id)
        if record.status != "running":
            return {"task": record.info(), "cancelled": False}
        if task_id not in self._cancellations:
            self._cancellations[task_id] = asyncio.create_task(self._cancel(record))
            self._cancellations[task_id].add_done_callback(
                lambda operation: operation.exception() if not operation.cancelled() else None
            )
        return await asyncio.shield(self._cancellations[task_id])

    async def _cancel(self, record: _Record) -> dict[str, Any]:
        self._suppress.add(record.task_id)
        cancelled = await self.kit.cancel_task(record.task_id)
        return {"task": record.info(), "cancelled": cancelled}

    async def wait(self, task_id: str) -> dict[str, Any]:
        """Host-only wait for relay attempt and resource cleanup, without cancelling work."""
        self._check(self.scope)
        record = self._record(task_id)
        await record.done.wait()
        if record.cleanup is not None:
            await asyncio.shield(record.cleanup)
        return record.info()

    async def aclose(self) -> None:
        """Permanently close; cleanup continues if the caller is cancelled."""
        self._check(self.scope)
        if self._close_task is None:
            self._closing = True
            self._close_task = asyncio.create_task(self._close())
        await asyncio.shield(self._close_task)

    async def _close(self) -> None:
        for launch in tuple(self._launches):
            launch.cancel()
        await asyncio.gather(*self._launches, return_exceptions=True)

        async def finish(record):
            try:
                if record.submitted and not record.done.is_set():
                    await self.kit.cancel_task(record.task_id)
            finally:
                await self._cleanup(record)

        try:
            outcomes = await asyncio.gather(
                *(finish(record) for record in tuple(self._records.values())),
                *self._cancellations.values(),
                return_exceptions=True,
            )
            failures = [outcome for outcome in outcomes if isinstance(outcome, BaseException)]
            if failures:
                raise BaseExceptionGroup("agent backend cleanup failed", failures)
        finally:
            for name in self._hook_names:
                self.kit.hook_engine.remove_global_hook(name)
            self._hook_names.clear()

    async def __aenter__(self) -> RoomKitAgentBackend:
        self._check(self.scope)
        if self._closing:
            raise AccessRefusedError("agent backend is closed")
        return self

    async def __aexit__(self, *args: Any) -> None:
        await self.aclose()
