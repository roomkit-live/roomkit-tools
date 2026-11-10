"""Mutation selection, shared first use and actual RoomKit agent execution."""

from __future__ import annotations

import asyncio
import hashlib
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
from roomkit.tools.compose import extract_tools
from roomkit_tools import (
    Edit,
    EditTool,
    InvalidInputError,
    Patch,
    PatchTool,
    Read,
    ReadTool,
    Shell,
    ShellTool,
    ToolIOError,
    Workspace,
    Write,
    WriteTool,
)
from text_helpers import content as page_lines


class RoomKitMutationTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self) -> None:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.workspace = Workspace(self.root)

    async def test_selection_schema_error_mapping_and_revision_chaining(self) -> None:
        definitions, handler = extract_tools(
            [
                Write(await WriteTool.create(self.workspace)),
                Edit(await EditTool.create(self.workspace)),
            ]
        )
        self.assertEqual([definition.name for definition in definitions], ["write", "edit"])
        for tool in (
            Write(await WriteTool.create(self.workspace)),
            Edit(await EditTool.create(self.workspace)),
            Patch(await PatchTool.create(self.workspace)),
        ):
            self.assertIs(tool.definition["changes_state"], True)
        for tool in (
            Read(await ReadTool.create(self.workspace)),
            Shell(await ShellTool.create(self.workspace)),
        ):
            self.assertNotIn("changes_state", tool.definition)
        self.assertEqual(definitions[0].parameters, WriteTool.definition()["input_schema"])
        self.assertEqual(definitions[1].parameters, EditTool.definition()["input_schema"])
        with self.assertRaises(UnservedToolCallError):
            await handler("read", {"path": "orders.txt"})
        first = json.loads(await handler("write", {"path": "orders.txt", "content": "value: 2\n"}))
        with self.assertRaises(ToolRefusedError):
            await handler("write", {"path": "orders.txt", "content": "new"})
        changed = json.loads(
            await handler(
                "edit",
                {
                    "path": "orders.txt",
                    "old_text": "2",
                    "new_text": "3",
                    "expected_revision": first["data"]["revision"],
                },
            )
        )
        self.assertNotEqual(changed["data"]["revision"], first["data"]["revision"])
        with self.assertRaises(ToolRefusedError):
            await handler(
                "edit",
                {
                    "path": "orders.txt",
                    "old_text": "3",
                    "new_text": "4",
                    "expected_revision": first["data"]["revision"],
                },
            )
        with self.assertRaises(ToolRefusedError):
            await handler("edit", {"path": "orders.txt", "old_text": "absent", "new_text": ""})
        with self.assertRaises(ToolRefusedError):
            await handler("write", {"path": "../outside.txt", "content": "new"})
        with self.assertRaises(ToolFailedError):
            await handler("write", {"path": "binary.txt", "content": "bad\0"})
        with self.assertRaises(ToolRefusedError):
            await handler("edit", {"path": "missing", "old_text": "a", "new_text": "b"})
        with self.assertRaises(ToolFailedError):
            await Write(await WriteTool.create(self.workspace, max_file_bytes=1)).handler(
                "write", {"path": "large.txt", "content": "ab"}
            )
        with self.assertRaises(InvalidInputError):
            await WriteTool.create(self.workspace, max_file_bytes=0)
        self.assertEqual((self.root / "orders.txt").read_text(), "value: 3\n")

    async def test_concurrent_first_calls_share_one_mutation_scope(self) -> None:
        before = b"old\n"
        (self.root / "file.txt").write_bytes(before)
        revision = "sha256:" + hashlib.sha256(before).hexdigest()
        write, edit = (
            Write(await WriteTool.create(self.workspace)),
            Edit(await EditTool.create(self.workspace)),
        )
        results = await asyncio.gather(
            write.handler(
                "write",
                {
                    "path": "file.txt",
                    "content": "written\n",
                    "overwrite": True,
                    "expected_revision": revision,
                },
            ),
            edit.handler(
                "edit",
                {
                    "path": "file.txt",
                    "old_text": "old",
                    "new_text": "edited",
                    "expected_revision": revision,
                },
            ),
            return_exceptions=True,
        )
        self.assertEqual(sum(isinstance(result, str) for result in results), 1)
        self.assertEqual(sum(isinstance(result, ToolRefusedError) for result in results), 1)
        self.assertIn((self.root / "file.txt").read_text(), {"written\n", "edited\n"})

    async def test_a_missing_root_can_be_retried_with_the_same_workspace(self) -> None:
        root = self.root / "later"
        workspace = Workspace(root)
        with self.assertRaises(ToolIOError):
            await WriteTool.create(workspace)
        root.mkdir()
        write, edit = (
            Write(await WriteTool.create(workspace)),
            Edit(await EditTool.create(workspace)),
        )
        await write.handler("write", {"path": "file.txt", "content": "old"})
        await edit.handler("edit", {"path": "file.txt", "old_text": "old", "new_text": "new"})
        self.assertEqual((root / "file.txt").read_text(), "new")

    async def test_actual_agent_turn_creates_edits_and_reads(self) -> None:
        content = "Order 43 - Alex: 2 items\n"
        revision = "sha256:" + hashlib.sha256(content.encode()).hexdigest()
        calls = [
            ("write", {"path": "orders.txt", "content": content}),
            (
                "edit",
                {
                    "path": "orders.txt",
                    "old_text": "2 items",
                    "new_text": "3 items",
                    "expected_revision": revision,
                },
            ),
            ("read", {"path": "orders.txt"}),
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
            + [AIResponse(content="Order updated and verified.")],
            streaming=False,
        )
        channel = AIChannel(
            "agent",
            provider=provider,
            tools=[
                Read(await ReadTool.create(self.workspace)),
                Write(await WriteTool.create(self.workspace)),
                Edit(await EditTool.create(self.workspace)),
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
                body="Create order 43, change it to three items, then verify the file."
            ),
        )
        try:
            output = await channel.on_event(event, binding, context)
            if output.response_stream is not None:
                async for _ in output.response_stream:
                    pass
        finally:
            await channel.close()
        self.assertEqual(len(provider.calls), 4)
        parts = [
            part
            for message in provider.calls[-1].messages
            if isinstance(message.content, list)
            for part in message.content
            if isinstance(part, AIToolResultPart)
        ]
        self.assertEqual(len(parts), 3)
        self.assertFalse(any(part.is_error for part in parts))
        results = [json.loads(part.result) for part in parts]
        self.assertEqual(results[0]["data"]["revision"], revision)
        self.assertEqual(results[1]["data"]["replacements"], 1)
        self.assertEqual(results[1]["data"]["revision"], results[2]["data"]["revision"])
        self.assertEqual(page_lines(results[2]["text"]), "1: Order 43 - Alex: 3 items\n")
        self.assertEqual((self.root / "orders.txt").read_text(), "Order 43 - Alex: 3 items\n")


class MutationLifecycleTests(unittest.TestCase):
    def test_adapters_and_shared_workspace_work_across_event_loops(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            workspace = Workspace(directory)
            write = Write(asyncio.run(WriteTool.create(workspace)))
            edit = Edit(asyncio.run(EditTool.create(workspace)))
            asyncio.run(write.handler("write", {"path": "file.txt", "content": "old"}))
            asyncio.run(
                edit.handler("edit", {"path": "file.txt", "old_text": "old", "new_text": "new"})
            )
            self.assertEqual(Path(directory, "file.txt").read_text(), "new")


if __name__ == "__main__":
    unittest.main()
