"""The optional adapter through RoomKit's actual channel and tool interfaces."""

from __future__ import annotations

import asyncio
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from roomkit import AIChannel
from roomkit.core.exceptions import ToolFailedError, ToolRefusedError, UnservedToolCallError
from roomkit.models.channel import ChannelBinding
from roomkit.models.context import RoomContext
from roomkit.models.enums import ChannelCategory, ChannelType
from roomkit.models.event import EventSource, RoomEvent, TextContent
from roomkit.models.room import Room
from roomkit.providers.ai.base import AIResponse, AIToolCall, AIToolResultPart
from roomkit.providers.ai.mock import MockAIProvider
from roomkit.tools.base import Tool
from roomkit.tools.compose import extract_tools
from roomkit_tools import Read, ReadTool, ToolIOError
from roomkit_tools.roomkit import ReadToolAdapter


class RoomKitTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self) -> None:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        (self.root / "source.txt").write_text("first\nsecond\n")
        self.tool = await ReadTool.create(self.root)
        self.adapter = ReadToolAdapter(self.tool)

    async def test_protocol_selection_schema_and_structured_result(self) -> None:
        self.assertIsInstance(self.adapter, Tool)
        definitions, handler = extract_tools([self.adapter])
        self.assertEqual(len(definitions), 1)
        self.assertEqual(definitions[0].parameters, self.tool.definition()["input_schema"])
        result = json.loads(await handler("read", {"path": "source.txt", "limit": 1}))
        self.assertEqual(result["text"], "1: first\n")
        self.assertEqual(result["data"]["next_offset"], 2)
        with self.assertRaises(UnservedToolCallError):
            await handler("write", {})

    async def test_refusal_failure_and_io_are_distinct(self) -> None:
        child = self.root / "child"
        child.mkdir()
        adapter = ReadToolAdapter(await ReadTool.create(child))
        with self.assertRaises(ToolRefusedError):
            await adapter.handler("read", {"path": "../source.txt"})
        for arguments in [{"path": "source.txt", "offset": 0}]:
            with self.assertRaises(ToolRefusedError):
                await self.adapter.handler("read", arguments)
        (self.root / "binary").write_bytes(b"\0")
        with self.assertRaises(ToolFailedError):
            await self.adapter.handler("read", {"path": "binary"})
        with self.assertRaises(ToolIOError):
            await self.adapter.handler("read", {"path": "missing"})

    async def test_direct_tool_protocol_schema_and_limits(self) -> None:
        tool = Read(self.root, max_lines=1)
        self.assertIsInstance(tool, Tool)
        definitions, handler = extract_tools([tool])
        self.assertEqual(definitions[0].parameters, ReadTool.definition()["input_schema"])
        result = json.loads(await handler("read", {"path": "source.txt", "limit": 100}))
        self.assertEqual(result["text"], "1: first\n")
        self.assertEqual(result["data"]["next_offset"], 2)
        with self.assertRaises(ToolFailedError):
            await Read(self.root, max_file_bytes=1).handler("read", {"path": "source.txt"})
        with self.assertRaises(ToolRefusedError):
            await Read(self.root, max_lines=0).handler("read", {"path": "source.txt"})

    async def test_initialization_failure_can_be_retried(self) -> None:
        child = self.root / "later"
        tool = Read(child)
        with self.assertRaises(UnservedToolCallError):
            await tool.handler("write", {})
        with self.assertRaises(ToolIOError):
            await tool.handler("read", {"path": "source.txt"})
        child.mkdir()
        (child / "source.txt").write_text("ready\n")
        self.assertEqual(
            json.loads(await tool.handler("read", {"path": "source.txt"}))["text"], "1: ready\n"
        )
        with self.assertRaises(ToolRefusedError):
            await tool.handler("read", {"path": "../source.txt"})
        with self.assertRaises(ToolRefusedError):
            await Read(child / "source.txt").handler("read", {"path": "source.txt"})

    async def test_relative_root_is_captured_before_first_call(self) -> None:
        cwd = Path.cwd()
        try:
            os.chdir(self.root)
            tool = Read(".")
        finally:
            os.chdir(cwd)
        self.assertEqual(
            json.loads(await tool.handler("read", {"path": "source.txt"}))["text"],
            "1: first\n2: second\n",
        )
        self.assertEqual(Path.cwd(), cwd)

    async def test_cancelled_initialization_does_not_poison_concurrent_calls(self) -> None:
        tool = Read(self.root)
        original_create = ReadTool.create
        started = asyncio.Event()
        proceed = asyncio.Event()

        async def delayed_create(*args, **kwargs):
            started.set()
            await proceed.wait()
            return await original_create(*args, **kwargs)

        with patch.object(ReadTool, "create", side_effect=delayed_create):
            first = asyncio.create_task(tool.handler("read", {"path": "source.txt"}))
            second = None
            try:
                await asyncio.wait_for(started.wait(), 5)
                second = asyncio.create_task(tool.handler("read", {"path": "source.txt"}))
                await asyncio.sleep(0)
                first.cancel()
                with self.assertRaises(asyncio.CancelledError):
                    await first
                proceed.set()
                result = json.loads(await asyncio.wait_for(second, 5))
                self.assertEqual(result["text"], "1: first\n2: second\n")
            finally:
                first.cancel()
                tasks = [first]
                if second is not None:
                    second.cancel()
                    tasks.append(second)
                await asyncio.gather(*tasks, return_exceptions=True)
        self.assertEqual(json.loads(await tool.handler("read", {"path": "source.txt"})), result)

    async def test_real_agent_turn_receives_the_native_result(self) -> None:
        provider = MockAIProvider(
            ai_responses=[
                AIResponse(
                    content="",
                    finish_reason="tool_calls",
                    tool_calls=[
                        AIToolCall(id="call-read", name="read", arguments={"path": "source.txt"})
                    ],
                ),
                AIResponse(content="The file has two lines."),
            ],
            streaming=False,
        )
        channel = AIChannel("agent", provider=provider, tools=[Read(root=self.root)])
        binding = ChannelBinding(
            room_id="test-room",
            channel_id="agent",
            channel_type=ChannelType.AI,
            category=ChannelCategory.INTELLIGENCE,
        )
        context = RoomContext(room=Room(id="test-room"), bindings=[binding])
        event = RoomEvent(
            room_id="test-room",
            source=EventSource(channel_id="user", channel_type=ChannelType.SMS),
            content=TextContent(body="Read source.txt"),
        )
        try:
            output = await channel.on_event(event, binding, context)
            if output.response_stream is not None:
                async for _ in output.response_stream:
                    pass
        finally:
            await channel.close()
        self.assertEqual(len(provider.calls), 2)
        parts = [
            part
            for message in provider.calls[-1].messages
            if isinstance(message.content, list)
            for part in message.content
            if isinstance(part, AIToolResultPart)
        ]
        self.assertEqual(len(parts), 1)
        result = json.loads(parts[0].result)
        self.assertEqual(result["text"], "1: first\n2: second\n")
        self.assertFalse(parts[0].is_error)

    async def test_handler_preserves_asyncio_cancellation(self) -> None:
        (self.root / "large.txt").write_bytes(b"line\n" * (6 * 1024 * 1024))
        adapter = Read(self.root, max_file_bytes=32 * 1024 * 1024)
        task = asyncio.create_task(adapter.handler("read", {"path": "large.txt"}))
        await asyncio.sleep(0)
        self.assertFalse(task.done())
        task.cancel()
        with self.assertRaises(asyncio.CancelledError):
            await task
        # A cancelled first call leaves the tool usable.
        result = json.loads(await adapter.handler("read", {"path": "source.txt"}))
        self.assertEqual(result["text"], "1: first\n2: second\n")


class ReadLifecycleTests(unittest.TestCase):
    def test_sync_construction_and_reuse_across_event_loops(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "source.txt"
            tool = Read(root=directory)
            for index in range(3):
                source.write_text(f"value {index}\n")
                result = json.loads(asyncio.run(tool.handler("read", {"path": "source.txt"})))
                self.assertEqual(result["text"], f"1: value {index}\n")


if __name__ == "__main__":
    unittest.main()
