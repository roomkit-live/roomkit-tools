"""Validate the coding example's real effects and reject false success claims."""

from __future__ import annotations

import asyncio
import importlib.util
import io
import tempfile
import unittest
from contextlib import redirect_stdout
from dataclasses import replace
from pathlib import Path
from unittest.mock import patch

from roomkit.providers.ai import AIResponse, AIToolCall, MockAIProvider

example_path = Path(__file__).resolve().parents[2] / "examples" / "roomkit_code.py"
spec = importlib.util.spec_from_file_location("roomkit_code_example", example_path)
assert spec is not None and spec.loader is not None
example = importlib.util.module_from_spec(spec)
spec.loader.exec_module(example)


class CodingScenarioTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self) -> None:
        directory = tempfile.TemporaryDirectory(prefix="roomkit-code-test-")
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        await asyncio.to_thread(example.prepare_project, self.root)

    async def test_chat_start_sends_one_request_and_check_does_not_call_the_model(self) -> None:
        provider = example.CodingProvider()
        output = io.StringIO()
        with patch("sys.stdin", io.StringIO("\n/start\n\n/check\nquit\n")), redirect_stdout(output):
            events = await example.run(self.root, provider, chat=True)
        self.assertEqual(len(provider.calls), 8)
        self.assertEqual(len(events), 7)
        self.assertEqual(output.getvalue().count("Agent turn finished."), 1)
        self.assertNotIn('Tool shell {"command":', output.getvalue())
        self.assertIn("Verified:", output.getvalue())

    async def test_real_repair_and_fake_test_command_rejection(self) -> None:
        with redirect_stdout(io.StringIO()):
            events = await example.run(self.root, example.CodingProvider())
        self.assertEqual(
            [e.name for e in events], ["shell", "grep", "read", "edit", "diff", "shell", "read"]
        )
        self.assertEqual((self.root / "shipping.py").read_text(), example.FIXED)
        self.assertEqual((self.root / "test_shipping.py").read_text(), example.TESTS)
        # Even a successful-looking shell result is invalid when its command
        # merely prints a report instead of running the required test suite.
        fake = replace(events[5], arguments={"command": "printf 'Ran 3 tests\\nOK\\n'"})
        with self.assertRaisesRegex(RuntimeError, "missing passing shell"):
            example.verify_trace([*events[:5], fake, *events[6:]], self.root)
        with self.assertRaisesRegex(RuntimeError, "missing baseline shell"):
            example.verify_trace(events[1:], self.root)
        # Reading before searching and inspecting the fix before rerunning tests
        # still satisfy the evidence requirements; absolute paths are valid too.
        equivalent = []
        for index in [2, 1, 0, 3, 6, 4, 5]:
            event = events[index]
            arguments = dict(event.arguments)
            for key in ("path", "old_path", "new_path"):
                if key in arguments:
                    arguments[key] = str(self.root / arguments[key])
            equivalent.append(replace(event, arguments=arguments))
        example.verify_trace(equivalent, self.root)

    async def test_prose_without_calls_does_not_validate_a_repair(self) -> None:
        provider = MockAIProvider(responses=["I corrected the bug and all three tests pass."])
        with redirect_stdout(io.StringIO()):
            with self.assertRaisesRegex(RuntimeError, "source correction is missing"):
                await example.run(self.root, provider)
        self.assertEqual((self.root / "shipping.py").read_text(), example.SOURCE)

    async def test_changing_tests_cannot_validate_the_buggy_source(self) -> None:
        provider = MockAIProvider(
            ai_responses=[
                AIResponse(
                    content="",
                    finish_reason="tool_calls",
                    tool_calls=[
                        AIToolCall(
                            id="weaken-test",
                            name="edit",
                            arguments={
                                "path": "test_shipping.py",
                                "old_text": "self.assertEqual(shipping_fee(50), 0)",
                                "new_text": "self.assertEqual(shipping_fee(50), 5)",
                            },
                        )
                    ],
                ),
                AIResponse(content="All tests pass now."),
            ]
        )
        with redirect_stdout(io.StringIO()):
            with self.assertRaisesRegex(RuntimeError, "test_shipping.py was changed"):
                await example.run(self.root, provider)
        self.assertIn(
            "self.assertEqual(shipping_fee(50), 5)", (self.root / "test_shipping.py").read_text()
        )
        self.assertEqual((self.root / "shipping.py").read_text(), example.SOURCE)


if __name__ == "__main__":
    unittest.main()
