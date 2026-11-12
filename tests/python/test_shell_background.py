"""A shell command runs on in the background as one of a process session's
jobs: started there, or moved there when it reaches its timeout."""

from __future__ import annotations

import asyncio
import gc
import tempfile
import time
import unittest
from pathlib import Path

from roomkit_tools import InvalidInputError, ProcessSession, ShellTool


async def next_end(ends):
    async with asyncio.timeout(5):
        return await anext(ends, None)


class ShellBackgroundTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self) -> None:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)

    async def session(self, **limits) -> ProcessSession:
        session = ProcessSession(self.root, **limits)
        self.addAsyncCleanup(session.close)
        return session

    async def moving_shell(self, session: ProcessSession, timeout_ms: int) -> ShellTool:
        return await ShellTool.create(
            self.root,
            processes=session,
            default_timeout_ms=timeout_ms,
            background_on_timeout=True,
        )

    async def stdout(self, session: ProcessSession, process_id: str, owner: str) -> str:
        read = (await session.read(process_id, owner=owner))["data"]
        self.assertEqual(read["status"]["state"], "finished")
        return read["stdout"]["text"]

    async def test_a_background_command_returns_at_once_as_a_process_of_its_owner(self):
        (self.root / "sub").mkdir()
        session = await self.session()
        ends = session.ends()
        shell = await ShellTool.create(self.root, processes=session, env={"GREETING": "hello"})
        started = time.monotonic()
        command = 'sleep 0.3; printf "$GREETING from $(basename "$PWD")"; cat; exit 3'
        result = await shell._call(
            {"command": command, "cwd": "sub", "run_in_background": True}, "agent"
        )
        self.assertLess(time.monotonic() - started, 0.25, "returned at once")
        data = result["data"]
        self.assertEqual(data["process_id"], "p1")
        self.assertEqual(
            (data["exit_code"], data["signal"], data["timed_out"]), (None, None, False)
        )
        self.assertEqual(data["cwd"], "sub")
        self.assertIn("process_read", result["text"])
        end = await next_end(ends)
        self.assertEqual(
            (end["owner"], end["tool"], end["process_id"], end["end"], end["exit_code"]),
            ("agent", "process", "p1", "exited", 3),
        )
        # The shell's environment and directory; stdin closed, as in the foreground.
        self.assertEqual(await self.stdout(session, "p1", "agent"), "hello from sub")

    async def test_a_background_command_needs_a_process_session(self):
        shell = await ShellTool.create(self.root)
        with self.assertRaisesRegex(InvalidInputError, "run_in_background"):
            await shell.shell("true", run_in_background=True)
        with self.assertRaisesRegex(InvalidInputError, "background_on_timeout"):
            await ShellTool.create(self.root, background_on_timeout=True)
        with self.assertRaises(TypeError):
            await ShellTool.create(self.root, processes=object())

    async def test_a_background_command_lives_within_the_process_tools_limits(self):
        session = await self.session(default_timeout_ms=10_000, max_timeout_ms=60_000)
        ends = session.ends()
        shell = await ShellTool.create(self.root, processes=session)
        # Above the shell's own maximum, within the process tools'.
        long = await shell.shell("true", timeout_ms=50_000, run_in_background=True)
        self.assertEqual(long["data"]["timeout_ms"], 50_000)
        self.assertEqual((await next_end(ends))["end"], "exited")
        with self.assertRaises(InvalidInputError):
            await shell.shell("true", timeout_ms=60_001, run_in_background=True)
        await shell.shell("sleep 30", timeout_ms=100, run_in_background=True)
        self.assertEqual((await next_end(ends))["end"], "timed_out")

    async def test_a_command_still_running_at_its_timeout_moves_to_the_background(self):
        session = await self.session()
        ends = session.ends()
        shell = await self.moving_shell(session, 300)
        command = "printf before; sleep 0.6; printf after; exit 4"
        result = await shell._call({"command": command}, "agent")
        data = result["data"]
        process_id = data["process_id"]
        self.assertEqual(
            (data["exit_code"], data["signal"], data["timed_out"]), (None, None, False)
        )
        self.assertEqual(data["stdout"]["text"], "before")
        self.assertIn(process_id, result["text"])
        end = await next_end(ends)
        self.assertEqual(
            (end["process_id"], end["end"], end["exit_code"]), (process_id, "exited", 4)
        )
        # Counted from the command's start, not from the move.
        self.assertGreaterEqual(end["elapsed_ms"], 550)
        self.assertEqual(await self.stdout(session, process_id, "agent"), "after")

    async def test_no_output_is_lost_or_repeated_when_a_command_moves(self):
        session = await self.session()
        ends = session.ends()
        shell = await self.moving_shell(session, 200)
        command = (
            "i=1; while [ $i -le 300 ]; do echo $i; echo e$i >&2; i=$((i+1)); sleep 0.002; done"
        )
        data = (await shell.shell(command))["data"]
        self.assertTrue(data["stdout"]["text"], "some lines before the move")
        await next_end(ends)
        read = (await session.read(data["process_id"]))["data"]
        self.assertEqual(
            data["stdout"]["text"] + read["stdout"]["text"],
            "".join(f"{i}\n" for i in range(1, 301)),
        )
        self.assertEqual(
            data["stderr"]["text"] + read["stderr"]["text"],
            "".join(f"e{i}\n" for i in range(1, 301)),
        )

    async def test_a_move_the_session_refuses_stops_the_command_as_before(self):
        session = await self.session(max_processes_per_owner=1)
        await session.start("sleep 30", owner="a")
        shell = await self.moving_shell(session, 200)
        result = await shell._call({"command": "printf before; sleep 30"}, "a")
        data = result["data"]
        self.assertIsNone(data["process_id"])
        self.assertTrue(data["timed_out"])
        self.assertEqual(data["signal"], 9)
        self.assertEqual(data["stdout"]["text"], "before")
        self.assertIn("not moved to the background", result["text"])

    async def test_the_shell_keeps_its_session_alive(self):
        session = ProcessSession(self.root)
        shell = await ShellTool.create(self.root, processes=session)
        del session
        gc.collect()
        result = await shell.shell("true", run_in_background=True)
        self.assertEqual(result["data"]["process_id"], "p1")
        await shell._processes.close()


if __name__ == "__main__":
    unittest.main()
