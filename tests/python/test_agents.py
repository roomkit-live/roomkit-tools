"""Portable agent tools use Rust validation, without importing RoomKit."""

import copy
import unittest

from roomkit_tools import (
    AgentFollowupTool,
    AgentScope,
    AgentSendTool,
    AskUserTool,
    CancelTaskTool,
    DelegateTaskTool,
    InvalidInputError,
    LimitExceededError,
    TaskStatusTool,
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
        receipt = await DelegateTaskTool(backend).call(
            {"profile": "reader", "task": "Read a file"}, scope=scope
        )
        self.assertEqual(receipt["data"], RECEIPT)
        result = await TaskStatusTool(backend).call({}, scope=scope)
        self.assertEqual(result["data"]["tasks"], [RECEIPT])
        self.assertEqual(backend.calls[-1], (scope, {"task_id": None}))
        cancelled = await CancelTaskTool(backend).call({"task_id": "t1"}, scope=scope)
        self.assertTrue(cancelled["data"]["cancelled"])

    async def test_rust_contract_rejects_input_before_backend(self):
        backend = Backend()
        tool = DelegateTaskTool(backend)
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
            await TaskStatusTool(backend).call({}, scope=AgentScope("room", "assistant"))
        backend.receipt.update(status="completed", result="x" * 8193)
        with self.assertRaises(LimitExceededError):
            await TaskStatusTool(backend).call({}, scope=AgentScope("room", "assistant"))

    def test_schemas_never_expose_host_scope_or_configuration(self):
        definition = DelegateTaskTool.definition()
        self.assertEqual(set(definition["input_schema"]["properties"]), {"profile", "task"})
        self.assertFalse(definition["input_schema"]["additionalProperties"])
        self.assertIn("delivery", definition["output_schema"]["properties"])

    async def test_followup_send_contracts_and_missing_backend_extensions(self):
        scope = AgentScope("room", "assistant")
        backend = Backend()
        for tool, arguments in (
            (AgentFollowupTool(backend), {"agent_id": "a1", "task": "Continue"}),
            (AgentSendTool(backend), {"agent_id": "a1", "message": "Clarify"}),
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
        result = await AgentFollowupTool(backend).call(
            {"agent_id": "a1", "task": "Continue"}, scope=scope
        )
        self.assertEqual(result["data"]["task_id"], "t2")
        result = await AgentSendTool(backend).call(
            {"agent_id": "a1", "message": "Clarify"}, scope=scope
        )
        self.assertEqual(result["data"]["message_id"], "m1")
        with self.assertRaises(LimitExceededError):
            await AgentSendTool(backend).call(
                {"agent_id": "a1", "message": "é" * 4097}, scope=scope
            )
        self.assertEqual(
            set(AgentSendTool.definition()["input_schema"]["properties"]), {"agent_id", "message"}
        )


class Person:
    """A backend whose person picks each question's first option."""

    def __init__(self):
        self.asked = []

    async def ask(self, scope, *, questions):
        self.asked.append((scope, questions))
        return {
            "answers": [
                {"selected": [q["options"][0]["label"]] if q["options"] else [], "other": None}
                for q in questions
            ]
        }


def question(text, *labels):
    return {"question": text, "options": [{"label": label} for label in labels]}


class AskUserTests(unittest.IsolatedAsyncioTestCase):
    def test_a_call_reads_with_its_defaults(self):
        read = AskUserTool.read(
            {
                "questions": [
                    question("Which database?", "SQLite", "Postgres"),
                    {"question": "Name?"},
                ]
            }
        )
        self.assertEqual(read["questions"][0]["options"][0], {"label": "SQLite", "description": ""})
        self.assertEqual(
            read["questions"][1],
            {"header": "", "question": "Name?", "options": [], "multi_select": False},
        )

    def test_a_call_is_refused_with_what_to_fix(self):
        refused = [
            (
                {"questions": [question(f"Q{n}?") for n in range(5)]},
                InvalidInputError,
                "ask_user takes one to 4 questions; 5 were given.",
            ),
            (
                {"questions": [question("A?"), question(" ")]},
                InvalidInputError,
                "Question 2 has no text.",
            ),
            (
                {"questions": [question("A?", "a", "b", "c", "d", "e")]},
                InvalidInputError,
                "Question 1 offers 5 options; at most 4.",
            ),
            (
                {"questions": [question("A?", "Yes", " Yes ")]},
                InvalidInputError,
                "Question 1 needs distinct, non-empty option labels.",
            ),
            (
                {"questions": [question("é" * 513)]},
                LimitExceededError,
                "Question 1's text exceeds 1024 bytes.",
            ),
        ]
        for arguments, error, message in refused:
            with self.subTest(message), self.assertRaises(error) as raised:
                AskUserTool.read(arguments)
            self.assertTrue(str(raised.exception).endswith(message), str(raised.exception))
        with self.assertRaises(InvalidInputError):
            AskUserTool.read({"questions": [{"question": "Q?", "kind": "x"}]})

    async def test_the_backend_answers_checked_questions(self):
        person = Person()
        scope = AgentScope("room", "assistant")
        result = await AskUserTool(person).call(
            {"questions": [question("Which database?", "SQLite", "Postgres")]}, scope=scope
        )
        self.assertEqual(result["data"], {"answers": [{"selected": ["SQLite"], "other": None}]})
        self.assertEqual(person.asked[0][0], scope)
        self.assertEqual(person.asked[0][1][0]["question"], "Which database?")
        with self.assertRaises(InvalidInputError):
            await AskUserTool(person).call({"questions": []}, scope=scope)
        self.assertEqual(len(person.asked), 1, "never reached the backend")
        with self.assertRaises(UnsupportedResourceError):
            await AskUserTool(Backend()).call({"questions": [question("Q?")]}, scope=scope)

    async def test_answers_are_bounded(self):
        class Talkative:
            async def ask(self, scope, *, questions):
                return {"answers": [{"selected": [], "other": "a" * 4097}]}

        with self.assertRaises(LimitExceededError):
            await AskUserTool(Talkative()).call(
                {"questions": [question("Q?")]}, scope=AgentScope("room", "assistant")
            )


class AgentSplitTests(unittest.TestCase):
    def test_each_tool_serves_its_call_and_definition_alone(self):
        """The core tool has `call` and a static `definition()`; RoomKit's
        `definition` property and `handler` belong to its adapter."""
        for cls in (
            DelegateTaskTool,
            TaskStatusTool,
            CancelTaskTool,
            AgentFollowupTool,
            AgentSendTool,
            AskUserTool,
        ):
            definition = cls.definition()
            self.assertEqual(
                set(definition), {"name", "description", "input_schema", "output_schema"}
            )
            self.assertFalse(hasattr(cls, "handler"))
            self.assertFalse(hasattr(cls, "contract"))
