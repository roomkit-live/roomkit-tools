"""RoomKit selects only the requested OCR facade, with no engine dependency."""

import tempfile
import unittest

from roomkit.core.exceptions import UnservedToolCallError
from roomkit.tools.compose import extract_tools
from roomkit_helpers import served
from roomkit_tools import (
    Ocr,
    OcrTool,
)
from test_ocr import Backend


class RoomKitOcrTests(unittest.IsolatedAsyncioTestCase):
    async def test_schema_result_and_tool_selection(self):
        backend = Backend()
        with tempfile.TemporaryDirectory() as root:
            definitions, handler = extract_tools(
                [Ocr(await OcrTool.create(root, backend=backend, max_blocks=4))]
            )
            self.assertEqual([d.name for d in definitions], ["ocr"])
            self.assertEqual(definitions[0].parameters, OcrTool.definition()["input_schema"])
            _, data = await served(handler, "ocr", {"path": "page.png"})
            self.assertEqual(data["text"], "fixture")
            self.assertEqual(backend.calls, [{"path": "page.png"}])
            with self.assertRaises(UnservedToolCallError):
                await handler("read", {"path": "page.png"})

    async def test_limits_set_on_the_tool_reach_the_backend(self):
        backend = Backend()
        with tempfile.TemporaryDirectory() as root:
            limits = {
                "max_file_bytes": 1,
                "max_dimension": 2,
                "max_pixels": 3,
                "max_decode_bytes": 4,
                "max_output_bytes": 5,
                "max_blocks": 6,
                "default_timeout_ms": 7,
                "max_timeout_ms": 8,
            }
            await Ocr(await OcrTool.create(root, backend=backend, **limits)).handler(
                "ocr", {"path": "page.png"}
            )
            await Ocr(await OcrTool.create(root, backend=backend, max_blocks=4)).handler(
                "ocr", {"path": "page.png"}
            )
            self.assertEqual([bound for _, bound in backend.bindings], [limits, {"max_blocks": 4}])
