"""Real RoomKit loop and independently selected fetch capability."""

import importlib.util
import io
import json
import unittest
from contextlib import redirect_stdout
from pathlib import Path

from roomkit.core.exceptions import ToolFailedError, ToolRefusedError, UnservedToolCallError
from roomkit.tools.compose import extract_tools
from roomkit_tools import (
    WebFetch,
    WebFetchTool,
)
from web_helpers import WebServer

EXAMPLE = Path(__file__).resolve().parents[2] / "examples/roomkit_web.py"


class RoomKitWebTests(unittest.IsolatedAsyncioTestCase):
    async def test_scripted_agent_fetches_a_real_page(self):
        spec = importlib.util.spec_from_file_location("web_example", EXAMPLE)
        example = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(example)
        with redirect_stdout(io.StringIO()) as output:
            events = await example.run()
        self.assertEqual([e.name for e in events], ["web_fetch"])
        self.assertIn("Verified:", output.getvalue())

    async def test_selected_tool_preserves_refusals_failures_and_http_status(self):
        async with WebServer() as server:
            tool = WebFetch(
                WebFetchTool(allowed_origins=[server.origin], private_origins=[server.origin])
            )
            definitions, handler = extract_tools([tool])
            self.assertEqual([d.name for d in definitions], ["web_fetch"])
            with self.assertRaises(UnservedToolCallError):
                await handler("shell", {"command": "true"})
            with self.assertRaises(ToolRefusedError):
                await handler("web_fetch", {"url": "https://example.com"})
            with self.assertRaises(ToolFailedError):
                await handler("web_fetch", {"url": server.origin + "/binary"})
            result = json.loads(await handler("web_fetch", {"url": server.origin + "/missing"}))
            self.assertEqual(result["data"]["status"], 404)
            self.assertFalse(result["data"]["http_ok"])
