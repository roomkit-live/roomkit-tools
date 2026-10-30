"""OCR selection works without installing any recognition engine."""

import importlib.util
import tempfile
import unittest
from pathlib import Path

from roomkit_tools import InvalidInputError, OcrTool, ReadTool


class Backend:
    def __init__(self):
        self.bindings = []
        self.calls = []

    async def bind(self, root, limits):
        self.bindings.append((root, limits))
        return self

    async def call(self, arguments):
        self.calls.append(arguments)
        return {"data": {"text": "fixture"}, "text": "fixture"}


class OcrTests(unittest.IsolatedAsyncioTestCase):
    async def test_selection_schema_and_host_backend_without_rusto(self):
        self.assertIsNone(importlib.util.find_spec("roomkit_tools_ocr_rusto"))
        self.assertEqual(ReadTool.definition()["name"], "read")
        schema = OcrTool.definition()
        self.assertEqual(schema["name"], "ocr")
        self.assertEqual(schema["input_schema"]["required"], ["path"])
        self.assertFalse(schema["input_schema"]["additionalProperties"])
        self.assertIn("blocks", schema["output_schema"]["properties"])
        backend = Backend()
        with tempfile.TemporaryDirectory() as root:
            tool = await OcrTool.create(root, backend=backend, max_blocks=3)
            result = await tool.ocr(Path("page.png"), rotation_degrees=90)
        self.assertEqual(result["data"]["text"], "fixture")
        self.assertEqual(backend.bindings, [(root, {"max_blocks": 3})])
        self.assertEqual(backend.calls[0]["path"], "page.png")
        self.assertEqual(backend.calls[0]["rotation_degrees"], 90)
        with self.assertRaises(InvalidInputError):
            await OcrTool.create("", backend=backend)
