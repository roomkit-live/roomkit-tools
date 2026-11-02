"""Individually selected diff/patch tools in RoomKit's actual agent loop."""

from __future__ import annotations

import asyncio
import hashlib
import json
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import AsyncMock

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
    Diff,
    DiffTool,
    Edit,
    EditTool,
    InvalidInputError,
    Patch,
    PatchTool,
    Read,
    ReadTool,
    ToolIOError,
    Workspace,
    Write,
    WriteTool,
)


class RoomKitDiffPatchTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self) -> None:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.workspace = Workspace(self.root)

    async def test_selection_schema_and_error_mapping(self) -> None:
        for facade, cls, name in [(Diff, DiffTool, "diff"), (Patch, PatchTool, "patch")]:
            definitions, handler = extract_tools([facade(await cls.create(self.workspace))])
            self.assertEqual([d.name for d in definitions], [name])
            self.assertEqual(definitions[0].parameters, cls.definition()["input_schema"])
            with self.assertRaises(UnservedToolCallError):
                await handler("write", {"path": "file", "content": "x"})
        diff, patch = (
            Diff(await DiffTool.create(self.workspace)),
            Patch(await PatchTool.create(self.workspace)),
        )
        (self.root / "old").write_text("old")
        (self.root / "new").write_text("new")
        with self.assertRaises(ToolRefusedError):
            await patch.handler(
                "patch",
                {
                    "changes": [{"op": "delete", "path": "old", "old_content": "stale"}],
                    "dry_run": False,
                },
            )
        with self.assertRaises(ToolRefusedError):
            await diff.handler("diff", {"old_path": "old", "new_path": ".."})
        with self.assertRaises(ToolIOError):
            await diff.handler("diff", {"old_path": "old", "new_path": "missing"})
        for facade, cls, args in [
            (Diff, DiffTool, {"old_path": "old", "new_path": "new"}),
            (
                Patch,
                PatchTool,
                {"changes": [{"op": "add", "path": "added", "content": "large"}], "dry_run": False},
            ),
        ]:
            limited = facade(await cls.create(self.workspace, max_file_bytes=1))
            with self.assertRaises(ToolFailedError):
                await limited.handler(facade.__name__.lower(), args)
            with self.assertRaises(InvalidInputError):
                await cls.create(self.workspace, max_file_bytes=0)
        self.assertEqual((self.root / "old").read_text(), "old")
        self.assertFalse((self.root / "added").exists())

    async def test_concurrent_first_calls_share_patch_write_edit_scope(self) -> None:
        (self.root / "file").write_text("old")
        rev = "sha256:" + hashlib.sha256(b"old").hexdigest()
        results = await asyncio.gather(
            Patch(await PatchTool.create(self.workspace)).handler(
                "patch",
                {
                    "changes": [
                        {
                            "op": "update",
                            "path": "file",
                            "old_content": "old",
                            "new_content": "patched",
                        }
                    ],
                    "dry_run": False,
                },
            ),
            Write(await WriteTool.create(self.workspace)).handler(
                "write",
                {"path": "file", "content": "written", "overwrite": True, "expected_revision": rev},
            ),
            Edit(await EditTool.create(self.workspace)).handler(
                "edit",
                {"path": "file", "old_text": "old", "new_text": "edited", "expected_revision": rev},
            ),
            return_exceptions=True,
        )
        self.assertEqual(sum(isinstance(r, str) for r in results), 1)
        self.assertEqual(sum(isinstance(r, ToolRefusedError) for r in results), 2)
        self.assertIn((self.root / "file").read_text(), {"patched", "written", "edited"})

    async def test_actual_agent_turn_compares_previews_applies_and_verifies(self) -> None:
        (self.root / "old.txt").write_text("count=2\n")
        (self.root / "new.txt").write_text("count=3\n")
        changes = [
            {
                "op": "update",
                "path": "old.txt",
                "old_content": "count=2\n",
                "new_content": "count=3\n",
            },
            {"op": "add", "path": "summary.txt", "content": "Count increased.\n"},
        ]
        calls = [
            ("diff", {"old_path": "old.txt", "new_path": "new.txt"}),
            ("patch", {"changes": changes}),
            ("patch", {"changes": changes, "dry_run": False}),
            ("diff", {"old_path": "old.txt", "new_path": "new.txt"}),
            ("read", {"path": "summary.txt"}),
        ]
        provider = MockAIProvider(
            ai_responses=[
                AIResponse(
                    content="",
                    finish_reason="tool_calls",
                    tool_calls=[AIToolCall(id=f"call-{index}", name=name, arguments=arguments)],
                )
                for index, (name, arguments) in enumerate(calls)
            ]
            + [AIResponse(content="Changes applied and verified.")],
            streaming=False,
        )
        channel = AIChannel(
            "agent",
            provider=provider,
            tools=[
                Diff(await DiffTool.create(self.workspace)),
                Patch(await PatchTool.create(self.workspace)),
                Read(await ReadTool.create(self.workspace)),
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
            content=TextContent(body="Compare, preview and apply the changes, then verify."),
        )
        try:
            output = await channel.on_event(event, binding, context)
            if output.response_stream is not None:
                async for _ in output.response_stream:
                    pass
        finally:
            await channel.close()
        self.assertEqual(len(provider.calls), 6)
        parts = [
            part
            for message in provider.calls[-1].messages
            if isinstance(message.content, list)
            for part in message.content
            if isinstance(part, AIToolResultPart)
        ]
        self.assertEqual(len(parts), 5)
        self.assertFalse(any(part.is_error for part in parts))
        results = [json.loads(part.result)["data"] for part in parts]
        self.assertTrue(results[0]["changed"])
        self.assertTrue(results[1]["dry_run"])
        self.assertEqual(results[1]["applied"], [])
        self.assertFalse(results[2]["dry_run"])
        self.assertTrue(results[2]["completed"])
        self.assertIsNone(results[2]["failure"])
        self.assertEqual(len(results[2]["applied"]), 2)
        self.assertFalse(results[3]["changed"])
        self.assertEqual(results[4]["lines"][0]["text"], "Count increased.")
        self.assertEqual((self.root / "old.txt").read_text(), "count=3\n")

    async def test_partial_result_is_preserved_and_cancellation_is_not_wrapped(self) -> None:
        # The Rust suite deterministically induces a conflict after preparation.
        # Here exercise the bridge/adapter with that shape: never turn partial
        # completion into a success-only envelope or discard committed effects.
        partial = {
            "data": {
                "dry_run": False,
                "completed": False,
                "previews": [],
                "applied": [{"index": 1, "operation": "add", "path": "first"}],
                "failure": {"index": 2, "cancelled": False, "message": "source changed"},
            },
            "text": "partial application",
        }
        native = SimpleNamespace(call=AsyncMock(return_value=json.dumps(partial)))
        facade = Patch(PatchTool(native))
        result = await facade.handler("patch", {"changes": [], "dry_run": False})
        self.assertEqual(json.loads(result), partial)
        native.call.side_effect = asyncio.CancelledError
        with self.assertRaises(asyncio.CancelledError):
            await facade.handler("patch", {"changes": []})
        self.assertTrue(native.call.call_args.args[1].is_cancelled())


class DiffPatchLifecycleTests(unittest.TestCase):
    def test_root_retry_and_cross_loop_use(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "later"
            workspace = Workspace(root)
            with self.assertRaises(ToolIOError):
                asyncio.run(PatchTool.create(workspace))
            root.mkdir()
            patch = Patch(asyncio.run(PatchTool.create(workspace)))
            diff = Diff(asyncio.run(DiffTool.create(workspace)))
            batch = {
                "changes": [{"op": "add", "path": "file", "content": "value"}],
                "dry_run": False,
            }
            asyncio.run(patch.handler("patch", batch))
            result = asyncio.run(diff.handler("diff", {"old_path": "file", "new_path": "file"}))
            self.assertFalse(json.loads(result)["data"]["changed"])


if __name__ == "__main__":
    unittest.main()
