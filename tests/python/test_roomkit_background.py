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
            self.assertEqual(len(demo.handoffs), 1)
            self.assertFalse(demo.posts)

    async def test_interactive_commands_only_use_the_speaker_for_returned_outcomes(self):
        example = load_example()

        class ResultSpeaker(example.SpeakerProvider):
            async def generate(self, context):
                text = str(context.messages[-1].content)
                if not text.startswith("[Instruction from the application:"):
                    raise AssertionError("Terminal commands must not ask the model to act")
                return await super().generate(context)

        lines = [
            "/start",
            "/start",
            "/status",
            "/list",
            "/check",
            "/cancel",
            "/check",
            "/cancel",
            "/start",
            "/cancel",
            "/check",
            "/unknown",
            "quit",
        ]
        with (
            tempfile.TemporaryDirectory() as root,
            redirect_stdout(io.StringIO()) as output,
            patch("builtins.input", side_effect=lines),
        ):
            async with asyncio.timeout(15):
                demo = await example.interactive(
                    Path(root), ResultSpeaker(), example.WorkerProvider(), True
                )
            self.assertEqual([t["end"] for t in demo.tasks.values()], ["cancelled", "cancelled"])
            self.assertEqual(len({t["child_room_id"] for t in demo.tasks.values()}), 2)
            self.assertEqual(len(demo.answers), 2)
            self.assertEqual(len(demo.deliveries), 2)
            self.assertTrue(all(e.channel_id == "researcher" for e in demo.tools))
            text = output.getvalue()
            self.assertIn("one task at a time", text)
            self.assertIn("verification pending", text)
            self.assertIn("nothing to cancel", text)
            self.assertIn("unknown command", text)
            self.assertEqual(text.count("Verified:"), 2)

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
            self.assertEqual(len(demo.handoffs), 1)
            self.assertEqual(
                next(iter(demo.handoffs.values())).metadata["task_status"], "cancelled"
            )
            self.assertEqual(output.getvalue().count("Verified:"), 1)

    async def test_host_commands_restart_after_completion_and_reject_unknown_tasks(self):
        example = load_example()
        with tempfile.TemporaryDirectory() as root, redirect_stdout(io.StringIO()) as output:
            demo = example.BackgroundDemo(Path(root), held=True)
            terminal = example.TerminalCommands(demo)
            try:
                async with asyncio.timeout(15):
                    await demo.open()
                    await terminal.handle("/start")
                    first = demo.latest()
                    child = first["child_room_id"]
                    await demo.until(lambda: child in demo.gate.entered, "worker not entered")
                    await terminal.handle("/status", first["task_id"])
                    self.assertIn("paused at code_outline", output.getvalue())
                    await terminal.handle("/cancel", "unknown")
                    self.assertIsNone(first["end"])
                    await terminal.handle("/release", first["task_id"])
                    await demo.until(lambda: demo.delivery_finished(first), "no relay")
                    await terminal.handle("/check", first["task_id"])
                    await terminal.handle("/cancel")
                    await terminal.handle("/release")
                    self.assertEqual(first["end"], "completed")
                    await terminal.handle("/start")
                    second = demo.latest()
                    self.assertNotEqual(first["task_id"], second["task_id"])
                    self.assertNotEqual(child, second["child_room_id"])
                    await terminal.handle("/cancel")
                    demo.verify_task(first)
                    demo.verify_task(second)
                    self.assertEqual(len(demo.deliveries), 2)
                    self.assertFalse(demo.posts)
                    self.assertIn("unknown task id", output.getvalue())
                    self.assertIn("nothing to cancel", output.getvalue())
                    self.assertIn("nothing to release", output.getvalue())
            finally:
                await demo.close()

    async def test_host_and_model_share_capacity_and_failed_start_releases_slot(self):
        example = load_example()
        with tempfile.TemporaryDirectory() as root, redirect_stdout(io.StringIO()):
            demo = example.BackgroundDemo(Path(root), held=True)
            try:
                async with asyncio.timeout(15):
                    await demo.open()
                    with patch.object(demo.kit, "start_task", side_effect=RuntimeError("failed")):
                        with self.assertRaisesRegex(RuntimeError, "failed"):
                            await demo.start()
                    self.assertIsNone(demo.reservation)
                    await demo.start()
                    await demo.say(example.START)
                    self.assertEqual(len(demo.tasks), 1)
                    self.assertIsNotNone(demo.reservation)
                    await demo.say(demo.cancel_request())
                    await demo.say(example.START)
                    with self.assertRaisesRegex(RuntimeError, "one task at a time"):
                        await demo.start()
                    self.assertEqual(len(demo.tasks), 2)
                    await demo.kit.cancel_task(demo.latest()["task_id"])
                    for _ in range(6):
                        task = await demo.start()
                        await demo.kit.cancel_task(task.id)
                    with self.assertRaisesRegex(RuntimeError, "eight tasks"):
                        await demo.start()
                    self.assertIsNone(demo.reservation)
            finally:
                await demo.close()

    async def test_automatic_mode_rejects_a_repeated_start_claim_without_a_new_task(self):
        example = load_example()

        class RepeatingSpeaker(example.SpeakerProvider):
            starts = 0

            async def generate(self, context):
                content = context.messages[-1].content
                if isinstance(content, str) and content.startswith(example.START):
                    self.starts += 1
                    if self.starts > 1:
                        return example.AIResponse(content="The task is running.")
                return await super().generate(context)

        speaker = RepeatingSpeaker()
        with tempfile.TemporaryDirectory() as root, redirect_stdout(io.StringIO()):
            async with asyncio.timeout(15):
                with self.assertRaisesRegex(RuntimeError, "no new task was observed"):
                    await example.automatic(Path(root), speaker=speaker)
                self.assertEqual(speaker.starts, 2)

    async def test_complete_and_cancel_with_real_child_tools_and_separate_chat(self):
        example = load_example()
        with tempfile.TemporaryDirectory() as root, redirect_stdout(io.StringIO()) as output:
            async with asyncio.timeout(15):
                demo = await example.automatic(Path(root))
            self.assertEqual([t["end"] for t in demo.tasks.values()], ["completed", "cancelled"])
            self.assertFalse(demo.posts)
            self.assertEqual(len(demo.handoffs), 1)
            self.assertEqual(len(demo.deliveries), 1)
            completed = next(iter(demo.tasks.values()))
            reply = demo.task_replies(completed)
            self.assertEqual(len(reply), 1)
            self.assertIn("The background analysis found:", reply[0].content.body)
            self.assertFalse(reply[0].content.body.startswith("Result of the task"))
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
                self.assertFalse(demo.handoffs)
                self.assertFalse(demo.deliveries)
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
            replies = list(demo.answers)
            demo.answers.clear()
            with self.assertRaisesRegex(RuntimeError, "no assistant reply"):
                demo.verify_task(completed)
            demo.answers[:] = replies
            demo.handoffs.clear()
            with self.assertRaisesRegex(RuntimeError, "exactly one result"):
                demo.verify_task(completed)
            await asyncio.to_thread(Path(root, "shipping.py").write_text, "changed\n")
            with self.assertRaisesRegex(RuntimeError, "source changed"):
                demo.verify_files()

    async def test_completed_worker_waits_for_assistant_reply_and_keeps_chat_separate(self):
        example = load_example()
        entered, release = asyncio.Event(), asyncio.Event()

        class SlowSpeaker(example.SpeakerProvider):
            async def generate(self, context):
                if str(context.messages[-1].content).startswith(
                    "[Instruction from the application:"
                ):
                    entered.set()
                    await release.wait()
                return await super().generate(context)

        with tempfile.TemporaryDirectory() as root, redirect_stdout(io.StringIO()) as output:
            demo = example.BackgroundDemo(Path(root), speaker=SlowSpeaker())
            try:
                async with asyncio.timeout(15):
                    await demo.open()
                    await demo.start()
                    await entered.wait()
                    task = demo.latest()
                    self.assertEqual(task["end"], "completed")
                    self.assertIn("assistant reply pending", demo.state(task))
                    await example.TerminalCommands(demo).handle("/check")
                    self.assertIn("verification pending", output.getvalue())
                    await demo.say(example.CHAT)
                    self.assertFalse(demo.task_replies(task))
                    self.assertTrue(demo.answers)
                    release.set()
                    await demo.until(lambda: demo.delivery_finished(task), "relay unfinished")
                    demo.verify_task(task)
                    replies = demo.task_replies(task)
                    self.assertEqual(len(replies), 1)
                    self.assertNotIn("A docstring documents", replies[0].content.body)
                    self.assertFalse(demo.posts)
            finally:
                await demo.close()

    async def test_empty_assistant_reply_does_not_validate_worker_completion(self):
        example = load_example()

        class SilentSpeaker(example.SpeakerProvider):
            async def generate(self, context):
                if str(context.messages[-1].content).startswith(
                    "[Instruction from the application:"
                ):
                    return example.AIResponse(content="")
                return await super().generate(context)

        with tempfile.TemporaryDirectory() as root, redirect_stdout(io.StringIO()):
            async with asyncio.timeout(15):
                with self.assertRaisesRegex(RuntimeError, "no assistant reply"):
                    await example.automatic(Path(root), speaker=SilentSpeaker())

    async def test_close_cancels_the_assistant_turn_receiving_the_result(self):
        example = load_example()
        entered, cancelled = asyncio.Event(), asyncio.Event()

        class HeldSpeaker(example.SpeakerProvider):
            async def generate(self, context):
                if str(context.messages[-1].content).startswith(
                    "[Instruction from the application:"
                ):
                    entered.set()
                    try:
                        await asyncio.Event().wait()
                    except asyncio.CancelledError:
                        cancelled.set()
                        raise
                return await super().generate(context)

        with tempfile.TemporaryDirectory() as root, redirect_stdout(io.StringIO()):
            demo = example.BackgroundDemo(Path(root), speaker=HeldSpeaker())
            try:
                async with asyncio.timeout(15):
                    await demo.open()
                    await demo.start()
                    await entered.wait()
                    await demo.close()
                    self.assertTrue(cancelled.is_set())
                    self.assertEqual(demo.latest()["end"], "completed")
                    self.assertFalse(demo.task_replies(demo.latest()))
                    self.assertFalse(demo.posts)
            finally:
                await demo.close()

    async def test_blocked_delivery_never_falls_back_to_posting_the_report(self):
        example = load_example()
        with tempfile.TemporaryDirectory() as root, redirect_stdout(io.StringIO()):
            demo = example.BackgroundDemo(Path(root))
            try:
                async with asyncio.timeout(15):
                    await demo.open()

                    @demo.kit.hook(example.HookTrigger.BEFORE_DELIVER)
                    async def refuse(event, context):
                        return example.HookResult.block("Blocked for this test")

                    await demo.start()
                    task = demo.latest()
                    await demo.until(lambda: demo.delivery_finished(task), "delivery unfinished")
                    self.assertEqual(task["end"], "completed")
                    self.assertFalse(demo.answers)
                    self.assertFalse(demo.posts)
                    with self.assertRaisesRegex(RuntimeError, "expected exactly one result"):
                        demo.verify_task(task)
            finally:
                await demo.close()
