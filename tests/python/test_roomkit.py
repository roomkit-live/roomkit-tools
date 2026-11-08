"""The optional adapter through RoomKit's actual channel and tool interfaces."""

from __future__ import annotations

import asyncio
import inspect
import json
import os
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
from roomkit_tools import (
    CodeOutline,
    CodeOutlineTool,
    Diff,
    DiffTool,
    Edit,
    EditTool,
    Glob,
    GlobTool,
    Grep,
    GrepTool,
    InvalidInputError,
    List,
    ListTool,
    Patch,
    PatchTool,
    Read,
    ReadTool,
    ToolIOError,
    ViewImage,
    ViewImageTool,
    Write,
    WriteTool,
)
from text_helpers import content


class RoomKitTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self) -> None:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        (self.root / "source.txt").write_text("first\nsecond\n")
        self.tool = await ReadTool.create(self.root)
        self.adapter = Read(self.tool)

    async def test_protocol_selection_schema_and_structured_result(self) -> None:
        self.assertIsInstance(self.adapter, Tool)
        definitions, handler = extract_tools([self.adapter])
        self.assertEqual(len(definitions), 1)
        self.assertEqual(definitions[0].parameters, self.tool.definition()["input_schema"])
        result = json.loads(await handler("read", {"path": "source.txt", "limit": 1}))
        self.assertEqual(content(result["text"]), "1: first\n")
        self.assertEqual(result["data"]["next_offset"], 2)
        with self.assertRaises(UnservedToolCallError):
            await handler("write", {})

    async def test_refusal_failure_and_io_are_distinct(self) -> None:
        child = self.root / "child"
        child.mkdir()
        adapter = Read(await ReadTool.create(child))
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

    async def test_adapters_take_a_built_tool(self) -> None:
        """Each adapter wraps a built tool, as Rust's `Read::new(tool)`: the
        tool takes the root and limits, never the adapter."""
        for adapter, standalone in [
            (ViewImage, ViewImageTool),
            (CodeOutline, CodeOutlineTool),
            (Read, ReadTool),
            (List, ListTool),
            (Glob, GlobTool),
            (Grep, GrepTool),
            (Diff, DiffTool),
            (Write, WriteTool),
            (Edit, EditTool),
            (Patch, PatchTool),
        ]:
            with self.subTest(adapter=adapter.__name__):
                self.assertEqual(list(inspect.signature(adapter).parameters), ["tool"])
                built = adapter(await standalone.create(self.root))
                self.assertEqual(built.definition["name"], standalone.definition()["name"])
                for wrong in (
                    self.root,
                    str(self.root),
                    self.tool if adapter is not Read else None,
                ):
                    with self.assertRaises(TypeError):
                        adapter(wrong)

    async def test_direct_tool_protocol_schema_and_limits(self) -> None:
        tool = Read(await ReadTool.create(self.root, max_lines=1))
        self.assertIsInstance(tool, Tool)
        definitions, handler = extract_tools([tool])
        self.assertEqual(definitions[0].parameters, ReadTool.definition()["input_schema"])
        result = json.loads(await handler("read", {"path": "source.txt", "limit": 100}))
        self.assertEqual(content(result["text"]), "1: first\n")
        self.assertEqual(result["data"]["next_offset"], 2)
        with self.assertRaises(ToolFailedError):
            await Read(await ReadTool.create(self.root, max_file_bytes=1)).handler(
                "read", {"path": "source.txt"}
            )
        # Invalid limits fail when the tool is built, before any adapter.
        with self.assertRaises(InvalidInputError):
            await ReadTool.create(self.root, max_lines=0)

    async def test_relative_root_is_captured_before_first_call(self) -> None:
        cwd = Path.cwd()
        try:
            os.chdir(self.root)
            tool = Read(await ReadTool.create("."))
        finally:
            os.chdir(cwd)
        self.assertEqual(
            content(json.loads(await tool.handler("read", {"path": "source.txt"}))["text"]),
            "1: first\n2: second\n",
        )
        self.assertEqual(Path.cwd(), cwd)

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
        channel = AIChannel(
            "agent", provider=provider, tools=[Read(await ReadTool.create(self.root))]
        )
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
        self.assertEqual(content(result["text"]), "1: first\n2: second\n")
        self.assertFalse(parts[0].is_error)

    async def test_handler_preserves_asyncio_cancellation(self) -> None:
        (self.root / "large.txt").write_bytes(b"line\n" * (6 * 1024 * 1024))
        adapter = Read(await ReadTool.create(self.root, max_file_bytes=32 * 1024 * 1024))
        task = asyncio.create_task(adapter.handler("read", {"path": "large.txt"}))
        await asyncio.sleep(0)
        self.assertFalse(task.done())
        task.cancel()
        with self.assertRaises(asyncio.CancelledError):
            await task
        # A cancelled first call leaves the tool usable.
        result = json.loads(await adapter.handler("read", {"path": "source.txt"}))
        self.assertEqual(content(result["text"]), "1: first\n2: second\n")

    async def test_discovery_selection_schemas_and_error_mapping(self) -> None:
        for facade, standalone, name, arguments in [
            (List, ListTool, "list", {}),
            (Glob, GlobTool, "glob", {"pattern": "*.txt"}),
            (Grep, GrepTool, "grep", {"pattern": "second", "context_before": 1}),
        ]:
            with self.subTest(tool=name):
                definitions, handler = extract_tools([facade(await standalone.create(self.root))])
                self.assertEqual([definition.name for definition in definitions], [name])
                self.assertEqual(definitions[0].parameters, standalone.definition()["input_schema"])
                native = await standalone.create(self.root)
                self.assertEqual(
                    json.loads(await handler(name, arguments)), await native.call(arguments)
                )
                with self.assertRaises(UnservedToolCallError):
                    await handler("read", {"path": "source.txt"})
                with self.assertRaises(ToolRefusedError):
                    await handler(name, {**arguments, "offset": 0})
                with self.assertRaises(ToolRefusedError):
                    await handler(name, {**arguments, "path": ".."})
                with self.assertRaises(ToolIOError):
                    await handler(name, {**arguments, "path": "missing"})
                limited = facade(await standalone.create(self.root, max_output_bytes=1))
                with self.assertRaises(ToolFailedError):
                    await limited.handler(name, arguments)
                with self.assertRaises(InvalidInputError):
                    await standalone.create(self.root, max_output_bytes=0)
        with self.assertRaises(ToolFailedError):
            await Grep(await GrepTool.create(self.root, max_context_lines=0)).handler(
                "grep", {"pattern": "second", "context_before": 1}
            )

    async def test_discovery_roots_are_independent(self) -> None:
        child = self.root / "later"
        for facade, standalone, name, arguments in [
            (List, ListTool, "list", {}),
            (Glob, GlobTool, "glob", {"pattern": "*.txt"}),
            (Grep, GrepTool, "grep", {"pattern": "second"}),
        ]:
            with self.assertRaises(ToolIOError):
                await standalone.create(child)
            child.mkdir()
            try:
                tool = facade(await standalone.create(child))
                outer = facade(await standalone.create(self.root))
                nested = json.loads(await tool.handler(name, arguments))
                outer = json.loads(await outer.handler(name, arguments))
                key = "matches" if name == "grep" else "entries"
                self.assertEqual(nested["data"][key], [])
                self.assertNotEqual(outer["data"][key], [])
            finally:
                child.rmdir()

    async def test_real_agent_turn_composes_all_four_tools(self) -> None:
        calls = [
            ("list", {}),
            ("glob", {"pattern": "**/*.txt"}),
            ("grep", {"pattern": "second", "context_before": 1}),
            ("read", {"path": "source.txt"}),
        ]
        provider = MockAIProvider(
            ai_responses=[
                AIResponse(
                    content="",
                    finish_reason="tool_calls",
                    tool_calls=[
                        AIToolCall(id=f"call-{name}", name=name, arguments=arguments),
                    ],
                )
                for name, arguments in calls
            ]
            + [AIResponse(content="The file has two lines.")],
            streaming=False,
        )
        channel = AIChannel(
            "agent",
            provider=provider,
            tools=[
                Read(await ReadTool.create(self.root)),
                List(await ListTool.create(self.root)),
                Glob(await GlobTool.create(self.root)),
                Grep(await GrepTool.create(self.root)),
            ],
        )
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
            content=TextContent(
                body="Discover files, search for second with context, then read it."
            ),
        )
        try:
            output = await channel.on_event(event, binding, context)
            if output.response_stream is not None:
                async for _ in output.response_stream:
                    pass
        finally:
            await channel.close()
        self.assertEqual(len(provider.calls), 5)
        parts = [
            part
            for message in provider.calls[-1].messages
            if isinstance(message.content, list)
            for part in message.content
            if isinstance(part, AIToolResultPart)
        ]
        self.assertEqual(len(parts), 4)
        self.assertFalse(any(part.is_error for part in parts))
        results = [json.loads(part.result) for part in parts]
        self.assertEqual(results[0]["data"]["entries"][0]["path"], "source.txt")
        self.assertEqual(results[1]["data"]["entries"][0]["path"], "source.txt")
        self.assertEqual(
            results[2]["data"]["matches"][0]["context_before"], [{"number": 1, "text": "first"}]
        )
        self.assertEqual(content(results[3]["text"]), "1: first\n2: second\n")


class ReadLifecycleTests(unittest.TestCase):
    def test_an_adapter_is_reused_across_event_loops(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "source.txt"
            tool = Read(asyncio.run(ReadTool.create(directory)))
            for index in range(3):
                source.write_text(f"value {index}\n")
                result = json.loads(asyncio.run(tool.handler("read", {"path": "source.txt"})))
                self.assertEqual(content(result["text"]), f"1: value {index}\n")

    def test_discovery_facades_can_be_used_across_event_loops(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            Path(directory, "source.txt").write_text("value\n")
            for facade, standalone, name, arguments in [
                (List, ListTool, "list", {}),
                (Glob, GlobTool, "glob", {"pattern": "*.txt"}),
                (Grep, GrepTool, "grep", {"pattern": "value"}),
            ]:
                tool = facade(asyncio.run(standalone.create(directory)))
                first = asyncio.run(tool.handler(name, arguments))
                second = asyncio.run(tool.handler(name, arguments))
                self.assertEqual(first, second)


if __name__ == "__main__":
    unittest.main()
