"""Installed plan contracts, scoped CAS storage and host backend injection."""

import asyncio
import json
import os
import unittest
from pathlib import Path

from roomkit_tools import (
    AccessRefusedError,
    InvalidInputError,
    LimitExceededError,
    MemoryPlanBackend,
    PlanCreateTool,
    PlanDeleteTool,
    PlanListTool,
    PlanReadTool,
    PlanScope,
    PlanUpdateTool,
    UnsupportedResourceError,
)

FIXTURE = json.loads(
    (
        Path(__file__).resolve().parents[2] / "crates/roomkit-tools-plans/tests/fixtures/plans.json"
    ).read_text()
)
CLASSES = (PlanCreateTool, PlanReadTool, PlanListTool, PlanUpdateTool, PlanDeleteTool)
CREATE = {"title": "Review shipping", "objective": "Check the threshold", "steps": []}
OWNER = PlanScope("plans", "assistant")


class PlanTests(unittest.IsolatedAsyncioTestCase):
    async def test_shared_rust_python_full_envelopes(self):
        backend = MemoryPlanBackend()
        tools = {cls.name: cls(backend) for cls in CLASSES}
        results = [await tools[c["tool"]].call(c["arguments"], scope=OWNER) for c in FIXTURE]
        self.assertEqual(results[3]["data"]["steps"][0]["step_id"], "test")
        self.assertEqual(results[4]["data"]["revision"], 2)
        self.assertEqual(results[-1]["data"]["plans"], [])
        reference = os.getenv("ROOMKIT_PLAN_RESULTS")
        if reference:
            self.assertEqual(
                results, json.loads(await asyncio.to_thread(Path(reference).read_text))
            )
        for cls in CLASSES:
            schema = cls.definition()["input_schema"]
            self.assertNotIn("room_id", schema.get("properties", {}))
            self.assertNotIn("caller_id", schema.get("properties", {}))
        self.assertIn("expected_revision", PlanDeleteTool.definition()["input_schema"]["required"])

    async def test_isolation_capacity_revision_conflicts_and_atomic_updates(self):
        backend = MemoryPlanBackend(max_plans=2, max_plans_per_scope=1)
        plan = await backend.create(OWNER, **CREATE)
        for outsider in (PlanScope("other", "assistant"), PlanScope("plans", "other")):
            self.assertEqual((await backend.list(outsider))["plans"], [])
            for operation in (
                backend.read(outsider, plan_id=plan["plan_id"]),
                backend.update(outsider, plan_id=plan["plan_id"], expected_revision=1, title="T"),
                backend.delete(outsider, plan_id=plan["plan_id"], expected_revision=1),
            ):
                with self.assertRaisesRegex(AccessRefusedError, "unknown plan in this scope"):
                    await operation
        with self.assertRaises(LimitExceededError):
            await backend.create(OWNER, **CREATE)
        results = await asyncio.gather(
            *(
                backend.update(OWNER, plan_id=plan["plan_id"], expected_revision=1, title=title)
                for title in ("First", "Second")
            ),
            return_exceptions=True,
        )
        self.assertEqual(sum(isinstance(r, AccessRefusedError) for r in results), 1)
        self.assertEqual((await backend.read(OWNER, plan_id=plan["plan_id"]))["revision"], 2)
        with self.assertRaisesRegex(AccessRefusedError, "revision conflict"):
            await backend.delete(OWNER, plan_id=plan["plan_id"], expected_revision=1)
        await backend.delete(OWNER, plan_id=plan["plan_id"], expected_revision=2)
        self.assertNotEqual((await backend.create(OWNER, **CREATE))["plan_id"], plan["plan_id"])

    async def test_backend_injection_validation_and_cancellation(self):
        class Host:
            calls = 0
            entered = asyncio.Event()
            stopped = asyncio.Event()
            invalid_result = False

            async def create(self, scope, **arguments):
                self.calls += 1
                self.owner = scope
                if self.invalid_result:
                    return {"plan_id": "bad"}
                self.entered.set()
                try:
                    await asyncio.Event().wait()
                finally:
                    self.stopped.set()

        host = Host()
        tool = PlanCreateTool(host)
        for invalid in ({**CREATE, "room_id": "spoof"}, {**CREATE, "title": ""}):
            with self.assertRaises(InvalidInputError):
                await tool.call(invalid, scope=OWNER)
        self.assertEqual(host.calls, 0)
        task = asyncio.create_task(tool.call(CREATE, scope=OWNER))
        await asyncio.wait_for(host.entered.wait(), 5)
        task.cancel()
        with self.assertRaises(asyncio.CancelledError):
            await task
        self.assertTrue(host.stopped.is_set())
        self.assertEqual(host.owner, OWNER)
        host.invalid_result = True
        with self.assertRaises(InvalidInputError):
            await tool.call(CREATE, scope=OWNER)
        with self.assertRaises(UnsupportedResourceError):
            await PlanListTool(host).call({}, scope=OWNER)

    async def test_step_changes_reach_a_host_backend_as_the_complete_steps(self):
        steps = [
            {"step_id": f"s{i}", "description": f"Step {i}", "status": "pending"}
            for i in range(1, 19)
        ]

        class Host:
            """A host backend written before step_changes existed."""

            plan = {"plan_id": "plan-1", "revision": 1, "title": "Release"}
            plan |= {"objective": "Ship", "steps": steps}

            async def read(self, scope, *, plan_id):
                return self.plan

            async def update(self, scope, *, plan_id, expected_revision, title, objective, steps):
                self.received = steps
                self.plan = {**self.plan, "revision": 2, "steps": steps}
                return self.plan

        host = Host()
        changes = [{"step_id": "s3", "status": "completed"}]
        arguments = {"plan_id": "plan-1", "expected_revision": 1, "step_changes": changes}
        result = await PlanUpdateTool(host).call(arguments, scope=OWNER)
        self.assertEqual(len(host.received), 18)
        self.assertEqual(host.received[2]["status"], "completed")
        self.assertEqual(host.received[3]["status"], "pending")
        self.assertEqual(
            result["text"], "plan-1 (revision 2): 1 step changed\n3. s3 [completed]: Step 3\n"
        )
        with self.assertRaises(InvalidInputError):
            await PlanUpdateTool(host).call(
                {**arguments, "step_changes": [{"step_id": "s99", "status": "completed"}]},
                scope=OWNER,
            )
        memory = MemoryPlanBackend()
        await memory.create(OWNER, title="Release", objective="Ship", steps=steps)
        direct = await memory.update(
            OWNER, plan_id="plan-1", expected_revision=1, step_changes=changes
        )
        self.assertEqual(direct["steps"][2]["status"], "completed")
        self.assertEqual(direct["revision"], 2)

    async def test_invalid_steps_and_revisions_leave_state_unchanged(self):
        backend = MemoryPlanBackend()
        tool = PlanCreateTool(backend)
        step = {"step_id": "inspect", "description": "Inspect", "status": "pending"}
        for steps in ([step, step], [{**step, "status": "invented"}], [{**step, "step_id": "a b"}]):
            with self.assertRaises(InvalidInputError):
                await tool.call({**CREATE, "steps": steps}, scope=OWNER)
        plan = await backend.create(OWNER, **CREATE)
        for patch in (
            {"title": "T"},
            {"expected_revision": 0, "title": "T"},
            {"expected_revision": 1},
        ):
            with self.assertRaises(InvalidInputError):
                await PlanUpdateTool(backend).call(
                    {"plan_id": plan["plan_id"], **patch}, scope=OWNER
                )
        self.assertEqual(await backend.read(OWNER, plan_id=plan["plan_id"]), plan)


class PlanSplitTests(unittest.TestCase):
    def test_each_tool_serves_its_call_and_definition_alone(self):
        """The core tool has `call` and a static `definition()`; RoomKit's
        `definition` property and `handler` belong to its adapter."""
        for cls in CLASSES:
            definition = cls.definition()
            self.assertEqual(
                set(definition), {"name", "description", "input_schema", "output_schema"}
            )
            self.assertFalse(hasattr(cls, "handler"))
            self.assertFalse(hasattr(cls, "contract"))


class CrossLoopTests(unittest.TestCase):
    def test_store_is_host_owned_and_not_event_loop_bound(self):
        backend = MemoryPlanBackend()
        plan = asyncio.run(backend.create(OWNER, **CREATE))
        self.assertEqual(asyncio.run(backend.read(OWNER, plan_id=plan["plan_id"])), plan)
        self.assertEqual(asyncio.run(MemoryPlanBackend().list(OWNER))["plans"], [])
