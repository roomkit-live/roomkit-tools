"""OCR selection works without installing any recognition engine."""

import importlib.util
import inspect
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


class NamedParametersTests(unittest.TestCase):
    """Each limit is a named keyword, the fields of Rust's OcrLimits: a misspelled one
    fails when it is passed, as any Python keyword."""

    def test_ocr_tool_create_names_every_limit(self):
        self.assertEqual(
            list(inspect.signature(OcrTool.create).parameters),
            [
                "root",
                "backend",
                "max_file_bytes",
                "max_dimension",
                "max_pixels",
                "max_decode_bytes",
                "max_output_bytes",
                "max_blocks",
                "default_timeout_ms",
                "max_timeout_ms",
            ],
        )
