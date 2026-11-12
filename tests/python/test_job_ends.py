"""A host is told when a background job ends, without polling it."""

from __future__ import annotations

import asyncio
import tempfile
import unittest
from pathlib import Path

from roomkit_tools import AccessRefusedError, ProcessSession, ShellTool


async def next_end(ends):
    async with asyncio.timeout(5):
        return await anext(ends, None)


class ProcessEndsTests(unittest.IsolatedAsyncioTestCase):
    async def test_an_exit_is_told_once_even_subscribed_before_first_use(self):
        with tempfile.TemporaryDirectory() as root:
            (Path(root) / "sub").mkdir()
            session = ProcessSession(root)
            # The native session is built at its first use, after this call.
            ends = session.ends()
            started = await session.start("sleep 0.2; exit 3", cwd="sub", owner="room-agent")
            end = await next_end(ends)
            self.assertEqual(
                end,
                {
                    "owner": "room-agent",
                    "tool": "process",
                    "process_id": started["data"]["process_id"],
                    "command": "sleep 0.2; exit 3",
                    "cwd": "sub",
                    "end": "exited",
                    "exit_code": 3,
                    "signal": None,
                    "elapsed_ms": end["elapsed_ms"],
                },
            )
            self.assertTrue(await session.close())
            self.assertIsNone(await next_end(ends))

    async def test_closing_tells_its_stops_then_ends_every_subscription(self):
        with tempfile.TemporaryDirectory() as root:
            session = ProcessSession(root)
            ends = session.ends()
            await session.start("sleep 30", owner="a")
            self.assertTrue(await session.close())
            self.assertEqual((await next_end(ends))["end"], "stopped")
            self.assertIsNone(await next_end(ends))
            self.assertIsNone(await next_end(session.ends()))

    async def test_a_session_closed_unused_ends_its_waiting_subscription(self):
        with tempfile.TemporaryDirectory() as root:
            session = ProcessSession(root)
            ends = session.ends()
            self.assertTrue(await session.close())
            self.assertIsNone(await next_end(ends))

    async def test_a_detached_job_is_let_go_and_not_told(self):
        with tempfile.TemporaryDirectory() as root:
            session = ProcessSession(root)
            ends = session.ends()
            await session.start("sleep 30", owner="a")
            self.assertTrue(await session.detach())
            self.assertIsNone(await next_end(ends))


class ShellEndsTests(unittest.IsolatedAsyncioTestCase):
    async def test_a_command_s_jobs_are_told_once_they_all_ended(self):
        with tempfile.TemporaryDirectory() as root:
            shell = await ShellTool.create(root, keep_background_jobs=True)
            ends = shell.ends()
            command = "sleep 0.3 >/dev/null 2>&1 & sleep 0.8 >/dev/null 2>&1 & echo started"
            result = await shell._call({"command": command}, "room-agent")
            self.assertEqual(result["data"]["exit_code"], 0)
            end = await next_end(ends)
            self.assertEqual(
                end,
                {
                    "owner": "room-agent",
                    "tool": "shell",
                    "process_id": None,
                    "command": command,
                    "cwd": ".",
                    "end": "exited",
                    "exit_code": None,
                    "signal": None,
                    "elapsed_ms": end["elapsed_ms"],
                },
            )
            self.assertGreaterEqual(end["elapsed_ms"], 800)
            self.assertTrue(await shell.close())
            self.assertIsNone(await next_end(ends))

    async def test_closing_stops_the_jobs_tells_them_then_refuses_commands(self):
        with tempfile.TemporaryDirectory() as root:
            shell = await ShellTool.create(root, keep_background_jobs=True)
            ends = shell.ends()
            await shell.shell("sleep 30 >/dev/null 2>&1 &")
            self.assertTrue(await shell.close())
            self.assertEqual((await next_end(ends))["end"], "stopped")
            self.assertIsNone(await next_end(ends))
            with self.assertRaises(AccessRefusedError):
                await shell.shell("true")

    async def test_detaching_lets_the_jobs_run_on_untold(self):
        with tempfile.TemporaryDirectory() as root:
            shell = await ShellTool.create(root, keep_background_jobs=True)
            ends = shell.ends()
            await shell.shell("(sleep 0.5; touch left) >/dev/null 2>&1 &")
            self.assertTrue(await shell.detach())
            self.assertIsNone(await next_end(ends))
            await asyncio.sleep(1.5)
            self.assertTrue((Path(root) / "left").exists())


if __name__ == "__main__":
    unittest.main()
