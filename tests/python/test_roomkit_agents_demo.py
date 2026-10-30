"""Parallel example and observed empty/error/limited turn diagnostics."""

import asyncio
import importlib.util
import io
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path

from roomkit import (
    AIChannel,
    ChannelCategory,
    InboundMessage,
    RoomKit,
    TextContent,
    WebSocketChannel,
)
from roomkit.providers.ai import AIResponse, MockAIProvider

EXAMPLE = Path(__file__).resolve().parents[2] / "examples/roomkit_agents.py"


def load_example():
    spec = importlib.util.spec_from_file_location("agents_example", EXAMPLE)
    example = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(example)
    return example


class AgentExampleTests(unittest.IsolatedAsyncioTestCase):
    async def test_parallel_profiles_preserve_targeted_message_and_cancel_only_reviewer(self):
        example = load_example()
        for dynamic in (False, True):
            with self.subTest(dynamic=dynamic), tempfile.TemporaryDirectory() as root:
                async with asyncio.timeout(20):
                    with redirect_stdout(io.StringIO()):
                        evidence = await example.run(Path(root), dynamic=dynamic, parallel=True)
                tasks = evidence["tasks"]
                researcher = [r for r in tasks if r["profile"] == "researcher"]
                reviewer = next(r for r in tasks if r["profile"] == "reviewer")
                self.assertEqual(len(researcher), 2)
                self.assertEqual(len({r["agent_id"] for r in researcher}), 1)
                self.assertEqual(len({r["task_id"] for r in researcher}), 2)
                self.assertTrue(all(r["delivery"] == "delivered" for r in researcher))
                self.assertTrue(all(evidence["marker"] in r["result"] for r in researcher))
                self.assertEqual(
                    (reviewer["status"], reviewer["delivery"]), ("cancelled", "suppressed")
                )
                self.assertNotEqual(reviewer["agent_id"], researcher[0]["agent_id"])
                self.assertEqual(evidence["message"]["agent_id"], researcher[0]["agent_id"])
                self.assertEqual((Path(root) / "shipping.py").read_text(), example.SOURCE)

    async def observe_turn(self, provider, tools=()):
        example = load_example()
        output = io.StringIO()
        async with asyncio.timeout(15), RoomKit() as kit:
            await kit.create_room(room_id="diagnostics")
            diagnostics = example.TurnDiagnostics(kit)
            kit.register_channel(AIChannel("assistant", provider=provider, tools=list(tools)))
            await kit.attach_channel(
                "diagnostics", "assistant", category=ChannelCategory.INTELLIGENCE
            )
            transport = WebSocketChannel("user")

            async def receive(_connection, _event):
                pass

            transport.register_connection("console", receive, room_id="diagnostics")
            kit.register_channel(transport)
            await kit.attach_channel("diagnostics", "user")
            with redirect_stdout(output):
                result = await kit.process_inbound(
                    InboundMessage(
                        channel_id="user", sender_id="developer", content=TextContent(body="Hello")
                    ),
                    room_id="diagnostics",
                )
        return list(diagnostics.records), output.getvalue(), result

    async def test_empty_response_is_observed_without_claiming_provider_error(self):
        records, output, _result = await self.observe_turn(MockAIProvider(responses=["   "]))
        self.assertEqual(len(records), 1)
        self.assertEqual(records[0]["kind"], "response")
        self.assertIn(records[0]["end_reason"], ("completed", "empty_response"))
        self.assertEqual(records[0]["text_chars"], 0)
        self.assertEqual(records[0]["tool_calls"], 0)
        self.assertIn('"text_chars": 0', output)

    async def test_reports_actual_roomkit_end_reason_for_limited_turns(self):
        normal, output, _ = await self.observe_turn(
            MockAIProvider(responses=["Hello"], streaming=True)
        )
        self.assertEqual(normal[-1]["end_reason"], "completed")
        self.assertEqual(output, "")
        example = load_example()

        class FinishTool:
            definition = {
                "name": "finish",
                "description": "Finish work",
                "parameters": {"type": "object", "properties": {}},
            }

            async def handler(self, name, arguments):
                return "done"

        class LimitedAfterTool(MockAIProvider):
            def __init__(self, final_text):
                super().__init__()
                self.final_text = final_text

            async def generate(self, context):
                self.calls.append(context)
                if len(self.calls) == 1:
                    return example.tool_call("finish", {})
                return AIResponse(content=self.final_text, finish_reason="length")

        limited, output, _ = await self.observe_turn(LimitedAfterTool(""), [FinishTool()])
        self.assertEqual(limited[-1]["end_reason"], "truncated")
        self.assertEqual(limited[-1]["text_chars"], 0)
        self.assertIn('"end_reason": "truncated"', output)

        # RoomKit 0.110 reports partial nonempty text as completed. Its public
        # event does not expose the raw provider finish reason: do not invent it.
        partial, output, _ = await self.observe_turn(
            LimitedAfterTool("An unfinished answer"), [FinishTool()]
        )
        self.assertEqual(partial[-1]["end_reason"], "completed")
        self.assertGreater(partial[-1]["text_chars"], 0)
        self.assertNotIn("An unfinished answer", output)

    async def test_provider_error_and_timeout_surface_without_raw_exception_body(self):
        for exception in (RuntimeError, TimeoutError):
            with self.subTest(exception=exception):

                class FailingProvider(MockAIProvider):
                    def __init__(self, error_type):
                        super().__init__()
                        self.error_type = error_type

                    async def generate(self, context):
                        raise self.error_type("SENSITIVE_FIXTURE_BODY")

                with self.assertLogs("roomkit", level="ERROR"):
                    records, output, result = await self.observe_turn(FailingProvider(exception))
                self.assertIsNotNone(result.error)
                errors = [r for r in records if r["kind"] == "error"]
                self.assertEqual(len(errors), 1)
                self.assertEqual(errors[0]["error_type"], exception.__name__)
                self.assertNotIn("SENSITIVE_FIXTURE_BODY", output)
                self.assertIn(exception.__name__, output)

    async def test_empty_final_answer_does_not_replay_a_completed_tool(self):
        example = load_example()
        effects = []

        class RecordTool:
            definition = {
                "name": "record",
                "description": "Record an effect",
                "parameters": {"type": "object", "properties": {}},
            }

            async def handler(self, name, arguments):
                effects.append("recorded")
                return "recorded"

        class EmptyAfterTool(MockAIProvider):
            async def generate(self, context):
                self.calls.append(context)
                if len(self.calls) == 1:
                    return example.tool_call("record", {})
                return AIResponse(content="", finish_reason="stop")

        records, output, _ = await self.observe_turn(EmptyAfterTool(), [RecordTool()])
        self.assertEqual(effects, ["recorded"])
        self.assertEqual(records[-1]["end_reason"], "empty_response")
        self.assertEqual(records[-1]["tool_calls"], 1)
        self.assertIn('"tool_calls": 1', output)
