"""Portable agent tools use Rust validation, without importing RoomKit."""

import copy
import unittest

from roomkit_tools import (
    AgentFollowup,
    AgentScope,
    AgentSend,
    CancelTask,
    DelegateTask,
    InvalidInputError,
    LimitExceededError,
    TaskStatus,
    UnsupportedResourceError,
)

RECEIPT = {
    "task_id": "t1",
    "agent_id": "a1",
    "profile": "reader",
    "status": "running",
    "delivery": "pending",
    "result": None,
    "result_truncated": False,
    "agent_available": True,
}


class Backend:
    def __init__(self):
        self.calls = []
        self.receipt = copy.deepcopy(RECEIPT)

    async def delegate(self, scope, **arguments):
        self.calls.append((scope, arguments))
        return self.receipt

    async def status(self, scope, **arguments):
        self.calls.append((scope, arguments))
        return {"tasks": [self.receipt]}

    async def cancel(self, scope, **arguments):
        self.calls.append((scope, arguments))
        self.receipt.update(status="cancelled", delivery="suppressed")
        return {"task": self.receipt, "cancelled": True}


class AgentTests(unittest.IsolatedAsyncioTestCase):
    async def test_individually_selected_tools_share_only_the_backend(self):
        backend = Backend()
        scope = AgentScope("room", "assistant")
        receipt = await DelegateTask(backend).call(
            {"profile": "reader", "task": "Read a file"}, scope=scope
        )
        self.assertEqual(receipt["data"], RECEIPT)
        result = await TaskStatus(backend).call({}, scope=scope)
        self.assertEqual(result["data"]["tasks"], [RECEIPT])
        self.assertEqual(backend.calls[-1], (scope, {"task_id": None}))
        cancelled = await CancelTask(backend).call({"task_id": "t1"}, scope=scope)
        self.assertTrue(cancelled["data"]["cancelled"])

    async def test_rust_contract_rejects_input_before_backend(self):
        backend = Backend()
        tool = DelegateTask(backend)
        for args in (
            {"profile": "reader", "task": " "},
            {"profile": "reader", "task": "read", "room_id": "foreign"},
            {"profile": 1, "task": "read"},
        ):
            with self.assertRaises(InvalidInputError):
                await tool.call(args, scope=AgentScope("room", "assistant"))
        with self.assertRaises(LimitExceededError):
            await tool.call(
                {"profile": "reader", "task": "é" * 8193}, scope=AgentScope("room", "assistant")
            )
        self.assertEqual(backend.calls, [])

    async def test_malformed_backend_results_are_refused(self):
        backend = Backend()
        backend.receipt["status"] = "invented"
        with self.assertRaises(InvalidInputError):
            await TaskStatus(backend).call({}, scope=AgentScope("room", "assistant"))
        backend.receipt.update(status="completed", result="x" * 8193)
        with self.assertRaises(LimitExceededError):
            await TaskStatus(backend).call({}, scope=AgentScope("room", "assistant"))

    def test_schemas_never_expose_host_scope_or_configuration(self):
        definition = DelegateTask.contract()
        self.assertEqual(set(definition["input_schema"]["properties"]), {"profile", "task"})
        self.assertFalse(definition["input_schema"]["additionalProperties"])
        self.assertIn("delivery", definition["output_schema"]["properties"])

    async def test_followup_send_contracts_and_missing_backend_extensions(self):
        scope = AgentScope("room", "assistant")
        backend = Backend()
        for tool, arguments in (
            (AgentFollowup(backend), {"agent_id": "a1", "task": "Continue"}),
            (AgentSend(backend), {"agent_id": "a1", "message": "Clarify"}),
        ):
            with self.assertRaises(UnsupportedResourceError):
                await tool.call(arguments, scope=scope)
            with self.assertRaises(InvalidInputError):
                await tool.call({**arguments, "room_id": "foreign"}, scope=scope)

        async def followup(scope, **arguments):
            return {**RECEIPT, "task_id": "t2"}

        async def send(scope, **arguments):
            return {
                "agent_id": "a1",
                "task_id": "t2",
                "message_id": "m1",
                "status": "queued",
                "history_recorded": True,
            }

        backend.followup, backend.send = followup, send
        result = await AgentFollowup(backend).call(
            {"agent_id": "a1", "task": "Continue"}, scope=scope
        )
        self.assertEqual(result["data"]["task_id"], "t2")
        result = await AgentSend(backend).call(
            {"agent_id": "a1", "message": "Clarify"}, scope=scope
        )
        self.assertEqual(result["data"]["message_id"], "m1")
        with self.assertRaises(LimitExceededError):
            await AgentSend(backend).call({"agent_id": "a1", "message": "é" * 4097}, scope=scope)
        self.assertEqual(
            set(AgentSend.contract()["input_schema"]["properties"]), {"agent_id", "message"}
        )
