"""Real RoomKit tools locate, read and fix a method using its native outline."""

import asyncio
import importlib.util
import io
import json
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path

from roomkit.core.exceptions import ToolFailedError, ToolRefusedError, UnservedToolCallError
from roomkit.tools.compose import extract_tools
from roomkit_tools import CodeOutline

EXAMPLE = Path(__file__).resolve().parents[2] / "examples/roomkit_outline.py"


class RoomKitOutlineTests(unittest.IsolatedAsyncioTestCase):
    async def test_example_uses_actual_ranges_revisions_and_preserves_other_methods(self):
        spec = importlib.util.spec_from_file_location("outline_example", EXAMPLE)
        example = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(example)
        with tempfile.TemporaryDirectory() as root, redirect_stdout(io.StringIO()) as output:
            events = await example.run(Path(root))
        self.assertEqual(
            [e.name for e in events],
            ["shell", "code_outline", "read", "edit", "diff", "shell", "read"],
        )
        self.assertTrue(all(not e.is_error for e in events))
        outline = json.loads(events[1].result)["data"]
        self.assertEqual(outline["total_symbols"], 5)
        self.assertEqual(len(json.loads(events[2].result)["data"]["lines"]), 5)
        self.assertIn("Verified:", output.getvalue())

    async def test_individual_selection_and_error_mapping(self):
        with tempfile.TemporaryDirectory() as root:
            await asyncio.to_thread(Path(root, "source.py").write_text, "def example(): pass\n")
            definitions, handler = extract_tools([CodeOutline(root)])
            self.assertEqual([d.name for d in definitions], ["code_outline"])
            with self.assertRaises(UnservedToolCallError):
                await handler("shell", {"command": "true"})
            with self.assertRaises(ToolRefusedError):
                await handler("code_outline", {"path": "source.py", "offset": 0})
            result = json.loads(await handler("code_outline", {"path": "source.py"}))
            self.assertEqual(result["data"]["symbols"][0]["name"], "example")
            _, limited = extract_tools([CodeOutline(root, max_file_bytes=1)])
            with self.assertRaises(ToolFailedError):
                await limited("code_outline", {"path": "source.py"})
