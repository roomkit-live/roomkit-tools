"""Persistent jobs selected individually and used across RoomKit agent turns."""

from __future__ import annotations

import asyncio
import json
import tempfile
import unittest
from pathlib import Path

from roomkit import AIChannel
from roomkit.core.exceptions import ToolFailedError, ToolRefusedError, UnservedToolCallError
from roomkit.models.channel import ChannelBinding
from roomkit.models.context import RoomContext
from roomkit.models.enums import ChannelCategory, ChannelType
from roomkit.models.event import EventSource, RoomEvent, TextContent
from roomkit.models.room import Room
from roomkit.providers.ai.base import AIContext, AIResponse, AIToolCall, AIToolResultPart
from roomkit.providers.ai.mock import MockAIProvider
from roomkit.tools.compose import extract_tools
from roomkit_tools import (
    ProcessRead,
    ProcessSession,
    ProcessStart,
    ProcessStatus,
    ProcessStop,
    ProcessWrite,
    ToolIOError,
)
from test_process import wait_status

ADAPTERS = (ProcessStart, ProcessStatus, ProcessRead, ProcessWrite, ProcessStop)


class RoomKitProcessTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self) -> None:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)

    async def test_individual_selection_and_rust_schemas(self) -> None:
        async with ProcessSession(self.root) as session:
            for adapter in ADAPTERS:
                tool = adapter(session)
                definitions, handler = extract_tools([tool])
                self.assertEqual(len(definitions), 1)
                name = definitions[0].name
                self.assertEqual(
                    definitions[0].parameters, session.definition(name)["input_schema"]
                )
                self.assertNotIn("owner", definitions[0].parameters["properties"])
                with self.assertRaises(UnservedToolCallError):
                    await handler("read", {"path": "notes"})

    async def test_failures_and_closed_session_map_to_roomkit_errors(self) -> None:
        session = ProcessSession(self.root, max_processes=1, max_processes_per_owner=1)
        async with session:
            _, handler = extract_tools([adapter(session) for adapter in ADAPTERS])
            for name, arguments in [
                ("process_start", {"command": ""}),
                ("process_start", {"command": "pwd", "owner": "other"}),
                ("process_start", {"command": "pwd", "cwd": ".."}),
                ("process_status", {"process_id": "missing"}),
                ("process_read", {"process_id": "missing"}),
                ("process_write", {"process_id": "missing", "text": "x"}),
                ("process_stop", {"process_id": "missing"}),
            ]:
                with self.subTest(name=name, arguments=arguments):
                    with self.assertRaises(ToolRefusedError):
                        await handler(name, arguments)
            with self.assertRaises(ToolIOError):
                await handler("process_start", {"command": "pwd", "cwd": "missing"})
            await handler("process_start", {"command": "cat"})
            with self.assertRaises(ToolFailedError):
                await handler("process_start", {"command": "cat"})
        with self.assertRaises(ToolRefusedError):
            await handler("process_start", {"command": "pwd"})

    async def test_agent_uses_all_five_tools_across_turns_without_implicit_eof(self) -> None:
        async with ProcessSession(self.root) as session:

            class ObservedProvider(MockAIProvider):
                async def generate(self, context: AIContext) -> AIResponse:
                    # Synchronize the scripted provider with real pipe capture,
                    # without changing the non-waiting process_read contract.
                    if len(self.calls) == 3:
                        await wait_status(session, "p1", stdout_bytes=8)
                    elif len(self.calls) == 6:
                        await wait_status(session, "p1", state="finished", output_complete=True)
                    return await super().generate(context)

            script = [
                ("process_start", {"command": "cat"}),
                None,
                ("process_write", {"process_id": "p1", "text": "Welcome\n"}),
                ("process_read", {"process_id": "p1"}),
                None,
                ("process_write", {"process_id": "p1", "close_stdin": True}),
                ("process_status", {"process_id": "p1"}),
                None,
                ("process_stop", {"process_id": "p1", "forget": True}),
                None,
            ]
            provider = ObservedProvider(
                ai_responses=[
                    AIResponse(
                        content="",
                        finish_reason="tool_calls",
                        tool_calls=[
                            AIToolCall(id=f"call-{index}", name=item[0], arguments=item[1])
                        ],
                    )
                    if item is not None
                    else AIResponse(content="Operation completed.")
                    for index, item in enumerate(script)
                ],
                streaming=False,
            )
            channel = AIChannel(
                "agent", provider=provider, tools=[adapter(session) for adapter in ADAPTERS]
            )
            binding = ChannelBinding(
                room_id="test-room",
                channel_id="agent",
                channel_type=ChannelType.AI,
                category=ChannelCategory.INTELLIGENCE,
            )
            context = RoomContext(room=Room(id="test-room"), bindings=[binding])
            results = []
            try:
                for request in ["Start cat", "Send and read", "Send EOF", "Forget the job"]:
                    response = await channel.on_event(
                        RoomEvent(
                            room_id="test-room",
                            source=EventSource(channel_id="user", channel_type=ChannelType.SMS),
                            content=TextContent(body=request),
                        ),
                        binding,
                        context,
                    )
                    if response.response_stream is not None:
                        async for _ in response.response_stream:
                            pass
                    parts = [
                        part
                        for message in provider.calls[-1].messages
                        if isinstance(message.content, list)
                        for part in message.content
                        if isinstance(part, AIToolResultPart)
                    ]
                    self.assertFalse(any(part.is_error for part in parts))
                    results.extend(json.loads(part.result)["data"] for part in parts)
            finally:
                await channel.close()
            self.assertEqual(len(provider.calls), 10)
            self.assertEqual(len(results), 6)
            start, write, read, eof, statuses, stop = results
            self.assertEqual(start["process_id"], "p1")
            self.assertTrue(write["complete"])
            self.assertEqual(write["bytes_written"], 8)
            self.assertFalse(write["stdin_closed"])
            self.assertEqual(read["stdout"]["bytes"], list(b"Welcome\n"))
            self.assertEqual(read["status"]["state"], "running")
            self.assertFalse(read["status"]["stdin_closed"])
            self.assertTrue(eof["stdin_closed"])
            self.assertEqual(statuses[0]["exit_code"], 0)
            self.assertIsNone(statuses[0]["signal"])
            self.assertEqual(statuses[0]["end"], "exited")
            self.assertTrue(stop["forgotten"])
            self.assertEqual((await session.status())["data"], [])

    async def test_concurrent_adapters_share_session_but_separate_sessions_are_isolated(
        self,
    ) -> None:
        async with ProcessSession(self.root) as first, ProcessSession(self.root) as second:
            results = await asyncio.gather(
                *[
                    ProcessStart(first).handler("process_start", {"command": "cat"})
                    for _ in range(3)
                ]
            )
            self.assertEqual(
                {json.loads(result)["data"]["process_id"] for result in results},
                {"p1", "p2", "p3"},
            )
            status = json.loads(await ProcessStatus(first).handler("process_status", {}))
            self.assertEqual(len(status["data"]), 3)
            with self.assertRaises(ToolRefusedError):
                await ProcessRead(second).handler("process_read", {"process_id": "p1"})
            self.assertEqual((await second.status())["data"], [])

    async def test_cancelled_handler_write_preserves_job_and_partial_stdin(self) -> None:
        async with ProcessSession(
            self.root, max_write_bytes=1_000_000, write_timeout_ms=5000
        ) as session:
            started = await session.start("sleep 60")
            process_id = started["data"]["process_id"]
            task = asyncio.create_task(
                ProcessWrite(session).handler(
                    "process_write", {"process_id": process_id, "text": "x" * 1_000_000}
                )
            )
            await asyncio.sleep(0.02)
            task.cancel()
            with self.assertRaises(asyncio.CancelledError):
                await task
            status = (await session.status(process_id))["data"][0]
            self.assertEqual(status["state"], "running")
            self.assertFalse(status["stdin_closed"])


if __name__ == "__main__":
    unittest.main()
