"""Host-selected documentation and structured RoomKit delivery."""

import asyncio
import json
import tempfile
import unittest
from pathlib import Path

from roomkit.core.exceptions import ToolRefusedError, UnservedToolCallError
from roomkit.tools.context import ToolCallContext, tool_turn_context
from roomkit_tools import DocsList, DocsRead, FileDocs


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
