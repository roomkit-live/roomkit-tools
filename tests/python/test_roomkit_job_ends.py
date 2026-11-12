"""An agent is told when a background job it started ends, through RoomKit."""

from __future__ import annotations

import asyncio
import tempfile
import unittest

from roomkit import AIChannel, InboundMessage, RoomKit, WebSocketChannel
from roomkit.models.enums import ChannelCategory
from roomkit.models.event import TextContent
from roomkit.providers.ai.base import AIResponse, AIToolCall
from roomkit.providers.ai.mock import MockAIProvider
from roomkit_tools import (
    ProcessSession,
    ProcessStart,
    Shell,
    ShellTool,
    notify_job_ends,
    roomkit_owner,
)
from roomkit_tools.roomkit.ends import _caller, _duration, _notice, _quoted


def end(tool: str, how: str, exit_code: int | None = None, signal: int | None = None) -> dict:
    return {
        "owner": roomkit_owner("room", "agent"),
        "tool": tool,
        "process_id": "p3" if tool == "process" else None,
        "command": "cargo test\n  --all",
        "cwd": ".",
        "end": how,
        "exit_code": exit_code,
        "signal": signal,
        "elapsed_ms": 133_000,
    }


class NoticeTests(unittest.TestCase):
    def test_each_end_reads_as_the_rust_adapter_writes_it(self):
        cases = [
            (
                end("process", "exited", exit_code=0),
                "Background process p3 (`cargo test --all`) exited with code 0 after 2m13s. "
                "Read its output with process_read.",
            ),
            (
                end("process", "exited", signal=9),
                "Background process p3 (`cargo test --all`) was killed by signal 9 after 2m13s. "
                "Read its output with process_read.",
            ),
            (
                end("process", "timed_out", signal=9),
                "Background process p3 (`cargo test --all`) reached its time limit after 2m13s "
                "and was stopped. Read its output with process_read.",
            ),
            (
                end("process", "failed"),
                "Background process p3 (`cargo test --all`) failed after 2m13s; "
                "process_status says why.",
            ),
            (
                end("shell", "exited"),
                "The background jobs of the shell command `cargo test --all` ended after 2m13s. "
                "Their exit status is unknown: check what they left (logs, files, ports).",
            ),
        ]
        for job_end, expected in cases:
            self.assertEqual(_notice(job_end), expected)
        self.assertIsNone(_notice(end("process", "stopped", signal=9)))

    def test_a_long_command_is_cut_and_durations_read_as_spoken(self):
        quoted = _quoted("x" * 250)
        self.assertEqual(len(quoted), 203)
        self.assertTrue(quoted.endswith("…`"))
        spoken = [_duration(ms) for ms in (850, 42_000, 133_000, 3_900_000)]
        self.assertEqual(spoken, ["850ms", "42s", "2m13s", "1h05m"])

    def test_only_a_roomkit_owner_names_a_caller(self):
        self.assertEqual(_caller(roomkit_owner("room", "agent")), ("room", "agent"))
        self.assertIsNone(_caller(None))
        self.assertIsNone(_caller("standalone"))


async def asked(provider: MockAIProvider, count: int) -> list[str]:
    async with asyncio.timeout(5):
        while True:
            if len(provider.calls) >= count:
                break
            await asyncio.sleep(0.02)
    return [repr(context) for context in provider.calls]


def call(name: str, arguments: dict) -> AIResponse:
    return AIResponse(content="", tool_calls=[AIToolCall(id="c1", name=name, arguments=arguments)])


class NotifyJobEndsTests(unittest.IsolatedAsyncioTestCase):
    async def run_turn(self, kit: RoomKit, tools: list, provider: MockAIProvider, ends) -> None:
        kit.register_channel(AIChannel("assistant", provider=provider, tools=tools))
        kit.register_channel(WebSocketChannel("chat"))
        await kit.create_room(room_id="room")
        await kit.attach_channel("room", "chat")
        await kit.attach_channel("room", "assistant", category=ChannelCategory.INTELLIGENCE)
        self.telling = asyncio.create_task(notify_job_ends(kit, ends))
        message = InboundMessage(
            channel_id="chat", sender_id="alice", content=TextContent(body="Start it.")
        )
        await kit.process_inbound(message, room_id="room")

    async def test_a_process_s_exit_gives_its_idle_agent_a_turn(self):
        with tempfile.TemporaryDirectory() as root:
            async with ProcessSession(root) as session, RoomKit() as kit:
                provider = MockAIProvider(
                    ai_responses=[
                        call("process_start", {"command": "sleep 0.3"}),
                        AIResponse(content="Started."),
                        AIResponse(content="It ended."),
                    ]
                )
                await self.run_turn(kit, [ProcessStart(session)], provider, session.ends())
                requests = await asked(provider, 3)
                told = "Background process p1 (`sleep 0.3`) exited with code 0 after"
                self.assertIn(told, requests[2])
                self.assertIn("Read its output with process_read.", requests[2])
            await asyncio.wait_for(self.telling, 5)

    async def test_a_shell_command_s_background_jobs_are_told_once_ended(self):
        with tempfile.TemporaryDirectory() as root:
            shell = await ShellTool.create(root, keep_background_jobs=True)
            command = "sleep 0.4 >/dev/null 2>&1 &"
            async with RoomKit() as kit:
                provider = MockAIProvider(
                    ai_responses=[
                        call("shell", {"command": command}),
                        AIResponse(content="Started."),
                        AIResponse(content="Seen."),
                    ]
                )
                await self.run_turn(kit, [Shell(shell)], provider, shell.ends())
                requests = await asked(provider, 3)
                self.assertIn(
                    f"The background jobs of the shell command `{command}` ended after",
                    requests[2],
                )
                self.assertTrue(await shell.close())
            await asyncio.wait_for(self.telling, 5)

    async def test_one_agent_s_turn_does_not_hold_back_another_room_s_notice(self):
        with tempfile.TemporaryDirectory() as root:
            shell = await ShellTool.create(root)
            async with ProcessSession(root) as session, RoomKit() as kit:
                # Room a's notice gives a turn that runs a two-second command;
                # room b's notice comes meanwhile.
                provider = MockAIProvider(
                    ai_responses=[
                        call("shell", {"command": "sleep 2"}),
                        AIResponse(content="Room b's seen."),
                        AIResponse(content="Room a's seen."),
                    ]
                )
                kit.register_channel(
                    AIChannel("assistant", provider=provider, tools=[Shell(shell)])
                )
                kit.register_channel(WebSocketChannel("chat"))
                for room in ("room-a", "room-b"):
                    await kit.create_room(room_id=room)
                    await kit.attach_channel(room, "chat")
                    await kit.attach_channel(
                        room, "assistant", category=ChannelCategory.INTELLIGENCE
                    )
                self.telling = asyncio.create_task(notify_job_ends(kit, session.ends()))
                for room, command in (("room-a", "true"), ("room-b", "sleep 0.3")):
                    await session.start(command, owner=roomkit_owner(room, "assistant"))
                async with asyncio.timeout(1.5):
                    requests = await asked(provider, 2)
                self.assertIn("Background process p1 (`true`)", requests[0])
                self.assertIn("Background process p2 (`sleep 0.3`)", requests[1])
                await asked(provider, 3)
            await asyncio.wait_for(self.telling, 5)


if __name__ == "__main__":
    unittest.main()
