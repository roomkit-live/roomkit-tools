"""The optional adapter through RoomKit's actual channel and tool interfaces."""

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
from roomkit.providers.ai.base import AIResponse, AIToolCall, AIToolResultPart
from roomkit.providers.ai.mock import MockAIProvider
from roomkit.tools.base import Tool
from roomkit.tools.compose import extract_tools
from roomkit_tools import ReadTool, ToolIOError
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
        channel = AIChannel("agent", provider=provider, tools=[self.adapter])
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
        adapter = ReadToolAdapter(await ReadTool.create(self.root, max_file_bytes=32 * 1024 * 1024))
        task = asyncio.create_task(adapter.handler("read", {"path": "large.txt"}))
        await asyncio.sleep(0)
        self.assertFalse(task.done())
        task.cancel()
        with self.assertRaises(asyncio.CancelledError):
            await task


if __name__ == "__main__":
    unittest.main()
