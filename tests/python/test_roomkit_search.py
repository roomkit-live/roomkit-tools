"""Real RoomKit loop and independently selected fetch capability."""

import importlib.util
import io
import json
import unittest
from contextlib import redirect_stdout
from pathlib import Path

from roomkit.core.exceptions import ToolFailedError, ToolRefusedError, UnservedToolCallError
from roomkit.tools.compose import extract_tools
from roomkit_tools import WebSearch
from roomkit_tools.search import ExaSearch
from search_helpers import SearchServer

EXAMPLE = Path(__file__).resolve().parents[2] / "examples/roomkit_search.py"


class RoomKitSearchTests(unittest.IsolatedAsyncioTestCase):
    async def test_agent_searches_then_fetches_returned_source(self):
        spec = importlib.util.spec_from_file_location("web_example", EXAMPLE)
        example = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(example)
        with redirect_stdout(io.StringIO()) as output:
            events = await example.run()
        self.assertEqual([e.name for e in events], ["web_search", "web_fetch"])
        self.assertIn("Verified:", output.getvalue())

    async def test_agent_consumes_cited_answer_from_native_openai_backend(self):
        spec = importlib.util.spec_from_file_location("search_answer_example", EXAMPLE)
        example = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(example)
        with redirect_stdout(io.StringIO()):
            events = await example.run(service="fixture-openai")
        output = json.loads(events[0].result)
        self.assertEqual(output["data"]["payload"]["kind"], "answer")
        self.assertEqual(output["data"]["usage"]["search_requests"], 1)
        self.assertIn("[source 1](<http://127.0.0.1:", output["text"])
        self.assertEqual(events[1].name, "web_fetch")

    async def test_selected_tool_preserves_failures_and_does_not_register_fetch(self):
        async with SearchServer() as server:
            tool = WebSearch(
                backend=ExaSearch(
                    endpoint=server.origin + "/tool-error", private_origins=[server.origin]
                )
            )
            definitions, handler = extract_tools([tool])
            self.assertEqual([d.name for d in definitions], ["web_search"])
            with self.assertRaises(UnservedToolCallError):
                await handler("web_fetch", {"url": "https://example.com"})
            with self.assertRaises(ToolRefusedError):
                await handler("web_search", {"query": ""})
            with self.assertRaises(ToolFailedError):
                await handler("web_search", {"query": "q"})
