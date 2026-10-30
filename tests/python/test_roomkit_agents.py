"""Exercise real RoomKit orchestration, scoped tools and resource ownership."""

import asyncio
import json
import unittest
from unittest.mock import patch

from roomkit import (
    AIChannel,
    ChannelCategory,
    HookExecution,
    HookTrigger,
    RoomKit,
    WebSocketChannel,
)
from roomkit.core.exceptions import ToolRefusedError
from roomkit.providers.ai import AIResponse, MockAIProvider
from roomkit.tools.context import ToolCallContext, tool_turn_context
from roomkit_tools import (
    AccessRefusedError,
    AgentScope,
    CancelTask,
    DelegateTask,
    LimitExceededError,
    TaskStatus,
)
from roomkit_tools.roomkit_agents import AgentConfig, RoomKitAgentBackend


def bounded(test):
    async def run(self):
        async with asyncio.timeout(20):
            await test(self)

    return run


class Worker(MockAIProvider):
    def __init__(self, gate=None, answer="Observed result"):
        super().__init__()
        self.gate = gate
        self.entered = asyncio.Event()
        self.closed = 0
        self.answer = answer

    async def generate(self, context):
        self.calls.append(context)
        self.entered.set()
        if self.gate is not None:
            await self.gate.wait()
        return AIResponse(content=self.answer)

    async def close(self):
        self.closed += 1


class RoomKitAgentTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.kit = RoomKit()
        await self.kit.create_room(room_id="parent")
        self.speaker = Worker(answer="The worker reported its result.")
        self.kit.register_channel(AIChannel("assistant", provider=self.speaker))
        await self.kit.attach_channel("parent", "assistant", category=ChannelCategory.INTELLIGENCE)
        user = WebSocketChannel("user")

        async def receive(connection, event):
            pass

        user.register_connection("console", receive, room_id="parent")
        self.kit.register_channel(user)
        await self.kit.attach_channel("parent", "user")
        self.scope = AgentScope("parent", "assistant")
        self.backends = []
        self.workers = []
        self.events = []

        @self.kit.hook(HookTrigger.AFTER_BROADCAST, execution=HookExecution.ASYNC)
        async def observe(event, context):
            self.events.append(event)

    async def asyncTearDown(self):
        async with asyncio.timeout(20):
            for backend in self.backends:
                await backend.aclose()
            await self.kit.close()

    def config(self, gate=None, answer="Observed result"):
        def create():
            worker = Worker(gate, answer)
            self.workers.append(worker)
            return worker

        return AgentConfig(provider_factory=create, system_prompt="Read only.")

    def backend(self, **options):
        backend = RoomKitAgentBackend(self.kit, scope=self.scope, **options)
        self.backends.append(backend)
        return backend

    async def launch(self, backend, profile="reader"):
        return (
            await DelegateTask(backend).call(
                {"profile": profile, "task": "Inspect the source"}, scope=self.scope
            )
        )["data"]

    @bounded
    async def test_static_profiles_make_distinct_instances_and_relay_then_release(self):
        gate = asyncio.Event()
        profiles = {"reader": self.config(gate)}
        backend = self.backend(profiles=profiles)
        profiles.clear()  # Captured catalogue, unaffected by host dictionary mutations.
        first, second = await asyncio.gather(self.launch(backend), self.launch(backend))
        self.assertNotEqual(first["task_id"], second["task_id"])
        self.assertNotEqual(first["agent_id"], second["agent_id"])
        self.assertEqual(first["profile"], second["profile"])
        with self.assertRaises(LimitExceededError):
            await self.launch(backend)
        gate.set()
        for task in (first, second):
            result = await backend.wait(task["task_id"])
            self.assertEqual(result["status"], "completed")
            self.assertEqual(result["delivery"], "delivered")
            self.assertIsNone(self.kit.get_channel(task["agent_id"]))
        self.assertEqual([w.closed for w in self.workers], [1, 1])
        self.assertEqual(len(self.speaker.calls), 2)
        replies = [e for e in self.events if e.source.channel_id == "assistant" and e.responds_to]
        self.assertEqual(len(replies), 2)

    @bounded
    async def test_dynamic_resolver_gets_trusted_scope_once_and_rechecks_profile(self):
        requests = []

        async def resolve(request):
            requests.append(request)
            return self.config(answer=request.profile)

        backend = self.backend(resolver=resolve, allowed_profiles=("reader", "reviewer"))
        tool = DelegateTask(backend)
        self.assertEqual(
            tool.definition["parameters"]["properties"]["profile"]["enum"], ["reader", "reviewer"]
        )
        with self.assertRaises(AccessRefusedError):
            await self.launch(backend, "admin")
        for profile in ("reader", "reviewer"):
            receipt = await self.launch(backend, profile)
            result = await backend.wait(receipt["task_id"])
            self.assertEqual(result["result"], profile)
        self.assertEqual(len(requests), 2)
        self.assertTrue(all(request.scope == self.scope for request in requests))

    @bounded
    async def test_scoping_handler_context_and_idempotent_cancellation(self):
        backend = self.backend(profiles={"reader": self.config(asyncio.Event())})
        receipt = await self.launch(backend)
        await self.workers[0].entered.wait()
        for scope in (AgentScope("other", "assistant"), AgentScope("parent", "other")):
            for tool, args in (
                (TaskStatus(backend), {}),
                (CancelTask(backend), {"task_id": receipt["task_id"]}),
                (DelegateTask(backend), {"profile": "reader", "task": "read"}),
            ):
                with self.assertRaises(AccessRefusedError):
                    await tool.call(args, scope=scope)
        with self.assertRaises(ToolRefusedError):
            await TaskStatus(backend).handler("task_status", {})
        with tool_turn_context(
            room_id="parent",
            actor_id="user",
            call=ToolCallContext(room_id="parent", channel_id="assistant"),
        ):
            status = json.loads(await TaskStatus(backend).handler("task_status", {}))
            self.assertEqual(status["data"]["tasks"][0]["task_id"], receipt["task_id"])
            cancelled = json.loads(
                await CancelTask(backend).handler("cancel_task", {"task_id": receipt["task_id"]})
            )["data"]
        self.assertTrue(cancelled["cancelled"])
        self.assertEqual(cancelled["task"]["status"], "cancelled")
        self.assertEqual(cancelled["task"]["delivery"], "suppressed")
        self.assertEqual(self.speaker.calls, [])
        again = await backend.cancel(self.scope, task_id=receipt["task_id"])
        self.assertFalse(again["cancelled"])
        self.assertEqual(self.workers[0].closed, 1)

    @bounded
    async def test_resolver_failure_releases_capacity_and_close_cancels_resolution(self):
        entered, release = asyncio.Event(), asyncio.Event()
        requests = []

        async def resolve(request):
            requests.append(request)
            if len(requests) == 1:
                raise ValueError("configuration unavailable")
            entered.set()
            await release.wait()
            return self.config()

        backend = self.backend(resolver=resolve, allowed_profiles=("reader",), max_concurrent=1)
        with self.assertRaisesRegex(ValueError, "configuration unavailable"):
            await self.launch(backend)
        launch = asyncio.create_task(self.launch(backend))
        await entered.wait()
        self.assertEqual((await backend.status(self.scope))["pending_launches"], 1)
        with self.assertRaises(LimitExceededError):
            await self.launch(backend)
        await backend.aclose()
        with self.assertRaises(asyncio.CancelledError):
            await launch
        self.assertEqual(self.workers, [])
        with self.assertRaises(AccessRefusedError):
            await self.launch(backend)

    @bounded
    async def test_cancelled_launch_caller_can_recover_from_status(self):
        entered, release = asyncio.Event(), asyncio.Event()

        async def resolve(request):
            entered.set()
            await release.wait()
            return self.config(asyncio.Event())

        backend = self.backend(resolver=resolve, allowed_profiles=("reader",))
        launch = asyncio.create_task(self.launch(backend))
        await entered.wait()
        launch.cancel()
        with self.assertRaises(asyncio.CancelledError):
            await launch
        release.set()
        # Wait on an actual lifecycle event, not a guessed task duration.
        started = asyncio.Event()

        @self.kit.hook(HookTrigger.ON_TASK_DELEGATED, execution=HookExecution.ASYNC)
        async def observe(event, context):
            started.set()

        await started.wait()
        status = await backend.status(self.scope)
        self.assertEqual(len(status["tasks"]), 1)
        await backend.aclose()
        self.assertEqual((await backend.status(self.scope))["tasks"][0]["status"], "cancelled")
        self.assertEqual(self.workers[0].closed, 1)

    @bounded
    async def test_large_results_are_clipped_and_reported(self):
        backend = self.backend(
            profiles={"reader": self.config(answer="é" * 5000)}, max_tasks=1, max_concurrent=1
        )
        receipt = await self.launch(backend)
        await backend.wait(receipt["task_id"])
        result = (await TaskStatus(backend).call({}, scope=self.scope))["data"]["tasks"][0]
        self.assertEqual(len(result["result"].encode()), 8192)
        self.assertTrue(result["result_truncated"])
        with self.assertRaises(LimitExceededError):
            await self.launch(backend)

    @bounded
    async def test_launch_failure_closes_provider_and_allows_retry(self):
        backend = self.backend(profiles={"reader": self.config()}, max_concurrent=1)
        with patch.object(self.kit, "delegate", side_effect=RuntimeError("submission failed")):
            with self.assertRaisesRegex(RuntimeError, "submission failed"):
                await self.launch(backend)
        self.assertEqual(self.workers[0].closed, 1)
        self.assertEqual((await backend.status(self.scope))["pending_launches"], 0)
        receipt = await self.launch(backend)
        await backend.wait(receipt["task_id"])
        self.assertEqual(self.workers[1].closed, 1)

    @bounded
    async def test_close_survives_cancelled_waiter_and_releases_all_instances(self):
        closing, release = asyncio.Event(), asyncio.Event()

        class SlowClose(Worker):
            async def close(self):
                closing.set()
                await release.wait()
                await super().close()

        workers = []

        def create():
            worker = SlowClose(asyncio.Event())
            workers.append(worker)
            return worker

        backend = self.backend(profiles={"reader": AgentConfig(create, "Read only.")})
        await self.launch(backend)
        await self.launch(backend)
        close = asyncio.create_task(backend.aclose())
        await closing.wait()
        close.cancel()
        with self.assertRaises(asyncio.CancelledError):
            await close
        release.set()
        await backend.aclose()
        self.assertEqual([worker.closed for worker in workers], [1, 1])
        self.assertTrue(
            all(t["status"] == "cancelled" for t in (await backend.status(self.scope))["tasks"])
        )

    @bounded
    async def test_task_completion_does_not_claim_principal_reply_early(self):
        gate = asyncio.Event()
        self.speaker.gate = gate
        backend = self.backend(profiles={"reader": self.config()})
        receipt = await self.launch(backend)
        await self.speaker.entered.wait()
        observed = (await backend.status(self.scope))["tasks"][0]
        self.assertEqual(observed["status"], "completed")
        self.assertEqual(observed["delivery"], "pending")
        cancelled = await backend.cancel(self.scope, task_id=receipt["task_id"])
        self.assertFalse(cancelled["cancelled"])
        gate.set()
        self.assertEqual((await backend.wait(receipt["task_id"]))["delivery"], "delivered")

    @bounded
    async def test_resolver_deadline_do_not_create_providers(self):
        async def resolve(request):
            await asyncio.Event().wait()

        backend = self.backend(
            resolver=resolve, allowed_profiles=("reader",), resolver_timeout=0.01
        )
        with self.assertRaises(TimeoutError):
            await self.launch(backend)
        self.assertEqual(self.workers, [])
        self.assertEqual((await backend.status(self.scope))["pending_launches"], 0)

    @bounded
    async def test_unknown_tasks_and_failed_delivery_are_explicit(self):
        backend = self.backend(profiles={"reader": self.config()})
        with self.assertRaises(AccessRefusedError):
            await backend.cancel(self.scope, task_id="foreign")
        with self.assertRaises(AccessRefusedError):
            await backend.status(self.scope, task_id="foreign")
        await self.kit.detach_channel("parent", "user")
        receipt = await self.launch(backend)
        result = await backend.wait(receipt["task_id"])
        self.assertEqual(result["status"], "completed")
        self.assertEqual(result["delivery"], "failed")
