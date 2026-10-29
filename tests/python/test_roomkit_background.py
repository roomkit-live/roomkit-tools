"""RoomKit owns child tasks; real Rust tools execute only in the worker room."""

import asyncio
import importlib.util
import io
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import patch

EXAMPLE = Path(__file__).resolve().parents[2] / "examples/roomkit_background.py"


def load_example():
    spec = importlib.util.spec_from_file_location("background_example", EXAMPLE)
    example = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(example)
    return example


class RoomKitBackgroundTests(unittest.IsolatedAsyncioTestCase):
    async def test_worker_can_request_outline_and_grep_in_the_same_round(self):
        example = load_example()

        class ParallelWorker(example.WorkerProvider):
            async def generate(self, context):
                if not example.results(context, "code_outline"):
                    response = example.call("code_outline", {"path": "shipping.py"})
                    response.tool_calls.extend(
                        example.call(
                            "grep", {"path": "shipping.py", "pattern": "subtotal > 50"}
                        ).tool_calls
                    )
                    return response
                return await super().generate(context)

        with tempfile.TemporaryDirectory() as root, redirect_stdout(io.StringIO()):
            async with asyncio.timeout(15):
                demo = await example.automatic(Path(root), worker=ParallelWorker())
            self.assertEqual([t["end"] for t in demo.tasks.values()], ["completed", "cancelled"])
            self.assertEqual(len(demo.posts), 1)

    async def test_interactive_reports_promises_without_actual_tool_calls(self):
        example = load_example()
        with (
            tempfile.TemporaryDirectory() as root,
            redirect_stdout(io.StringIO()) as output,
            patch("builtins.input", side_effect=["/start", "/status", "quit"]),
        ):
            async with asyncio.timeout(15):
                demo = await example.interactive(
                    Path(root),
                    example.MockAIProvider(responses=["Done."]),
                    example.WorkerProvider(),
                    True,
                )
            self.assertFalse(demo.tasks)
            self.assertIn("no successful work_in_background call", output.getvalue())
            self.assertIn("no successful task_status call", output.getvalue())

    async def test_interactive_plain_input_reaches_the_same_parent_as_slash_commands(self):
        example = load_example()
        # CLIChannel routes ordinary input by participant; slash commands explicitly
        # name the room. Both paths must use the same participant identity.
        lines = ["/start", example.CHAT, "/status", "/cancel", "/check", "quit"]
        with (
            tempfile.TemporaryDirectory() as root,
            redirect_stdout(io.StringIO()) as output,
            patch("builtins.input", side_effect=lines),
        ):
            async with asyncio.timeout(15):
                demo = await example.interactive(
                    Path(root), example.SpeakerProvider(), example.WorkerProvider(), True
                )
            self.assertIn("A docstring documents", output.getvalue())
            self.assertEqual(len(demo.tasks), 1)
            self.assertEqual(demo.latest()["end"], "cancelled")
            self.assertFalse(demo.posts)
            self.assertEqual(output.getvalue().count("Verified:"), 1)

    async def test_complete_and_cancel_with_real_child_tools_and_separate_chat(self):
        example = load_example()
        with tempfile.TemporaryDirectory() as root, redirect_stdout(io.StringIO()) as output:
            async with asyncio.timeout(15):
                demo = await example.automatic(Path(root))
            self.assertEqual([t["end"] for t in demo.tasks.values()], ["completed", "cancelled"])
            self.assertEqual(len(demo.posts), 1)
            self.assertEqual(demo.turns.count(example.CHAT), 2)
            self.assertEqual(len(demo.gate.cancelled), 1)
            self.assertEqual(
                [t.definition["name"] for t in demo.worker_tools], ["code_outline", "grep", "read"]
            )
            task_rooms = {t["child_room_id"] for t in demo.tasks.values()}
            self.assertNotIn(example.ROOM, task_rooms)
            native_calls = [e for e in demo.tools if e.name in {"code_outline", "grep", "read"}]
            self.assertTrue(native_calls)
            self.assertTrue(all(e.room_id in task_rooms for e in native_calls))
            self.assertIn("Verified:", output.getvalue())

    async def test_close_cancels_pending_worker_and_cleans_up_without_releasing_gate(self):
        example = load_example()
        with tempfile.TemporaryDirectory() as root, redirect_stdout(io.StringIO()):
            demo = example.BackgroundDemo(Path(root), held=True)
            try:
                async with asyncio.timeout(10):
                    await demo.open()
                    await demo.say(example.START)
                    task = demo.latest()
                    child = task["child_room_id"]
                    await demo.until(lambda: child in demo.gate.entered, "worker not entered")
                    await demo.close()
                self.assertEqual(task["end"], "cancelled")
                self.assertIn(child, demo.gate.cancelled)
                self.assertFalse(demo.posts)
                self.assertFalse(demo.gate.releases[child].is_set())
            finally:
                await demo.close()

    async def test_refused_launch_does_not_release_the_running_task_slot(self):
        example = load_example()
        with tempfile.TemporaryDirectory() as root, redirect_stdout(io.StringIO()):
            demo = example.BackgroundDemo(Path(root), held=True)
            try:
                async with asyncio.timeout(10):
                    await demo.open()
                    await demo.say(example.START)
                    first = demo.latest()
                    # The mock deliberately reuses a tool call id across turns.
                    # Rejections must not free the earlier task's reservation.
                    await demo.say(example.START)
                    await demo.say(example.START)
                    self.assertEqual(len(demo.tasks), 1)
                    self.assertIsNotNone(demo.reservation)
                    refused = [
                        e for e in demo.tools if e.name == "work_in_background" and e.refused
                    ]
                    self.assertEqual(len(refused), 2)
                    await demo.say(demo.cancel_request())
                    self.assertEqual(first["end"], "cancelled")
                    await demo.say(example.START)
                    self.assertEqual(len(demo.tasks), 2)
                    self.assertNotEqual(demo.latest()["child_room_id"], first["child_room_id"])
            finally:
                await demo.close()

    async def test_verifier_rejects_missing_delivery_and_changed_source(self):
        example = load_example()
        with tempfile.TemporaryDirectory() as root, redirect_stdout(io.StringIO()):
            async with asyncio.timeout(15):
                demo = await example.automatic(Path(root))
            completed = next(iter(demo.tasks.values()))
            demo.posts.clear()
            with self.assertRaisesRegex(RuntimeError, "exactly one result"):
                demo.verify_task(completed)
            await asyncio.to_thread(Path(root, "shipping.py").write_text, "changed\n")
            with self.assertRaisesRegex(RuntimeError, "source changed"):
                demo.verify_files()
