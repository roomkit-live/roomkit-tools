"""Host-selected documentation and structured RoomKit delivery."""

import asyncio
import json
import tempfile
import unittest
from pathlib import Path

from roomkit.core.exceptions import ToolFailedError, ToolRefusedError, UnservedToolCallError
from roomkit.tools.context import ToolCallContext, tool_turn_context
from roomkit_tools import (
    DocsList,
    DocsRead,
    FileDocs,
    LimitExceededError,
    ToolError,
    ToolIOError,
    UnsupportedResourceError,
)


class RoomKitDocsTests(unittest.IsolatedAsyncioTestCase):
    async def test_selection_refusals_and_structured_output(self):
        with tempfile.TemporaryDirectory() as temporary:
            await asyncio.to_thread(
                Path(temporary, "guide.md").write_text, "# Guide\nRead reference material.\n"
            )
            backend = FileDocs(temporary, version="demo")
            lister, reader = DocsList(backend), DocsRead(backend)
            call = ToolCallContext(room_id="docs", channel_id="assistant")
            with tool_turn_context(room_id="docs", actor_id="user", call=call):
                result = json.loads(await lister.handler("docs_list", {}))
                self.assertEqual(call.structured_content, result)
                self.assertEqual(result["data"]["documents"][0]["version"], "demo")
                with self.assertRaises(UnservedToolCallError):
                    await reader.handler("read", {"document_id": "guide.md"})
                with self.assertRaises(ToolRefusedError):
                    await reader.handler("docs_read", {"document_id": "unknown.md"})
                with self.assertRaises(ToolRefusedError):
                    await reader.handler("docs_read", {"document_id": "guide.md", "offset": 0})
                result = json.loads(await reader.handler("docs_read", {"document_id": "guide.md"}))
                self.assertEqual(call.structured_content, result)
                self.assertEqual(result["data"]["lines"][1]["text"], "Read reference material.")

    async def test_errors_map_like_the_other_roomkit_adapters(self):
        class Failing:
            def __init__(self, error):
                self.error = error

            async def list(self, **arguments):
                raise self.error

        with tempfile.TemporaryDirectory() as temporary:
            # I/O failures propagate for RoomKit to report without their message.
            with self.assertRaises(ToolIOError):
                await DocsList(FileDocs(Path(temporary, "missing"))).handler("docs_list", {})
        with self.assertRaises(ToolIOError):
            await DocsList(Failing(ToolIOError("disk"))).handler("docs_list", {})
        with self.assertRaises(ToolError) as raised:
            await DocsList(Failing(ToolError("other"))).handler("docs_list", {})
        self.assertIs(type(raised.exception), ToolError)
        for error in [LimitExceededError("budget"), UnsupportedResourceError("kind")]:
            with self.subTest(error=type(error).__name__), self.assertRaises(ToolFailedError):
                await DocsList(Failing(error)).handler("docs_list", {})
