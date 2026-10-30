"""Installed optional engine: contracts, typed failures and real-model parity."""

import asyncio
import hashlib
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from roomkit.core.exceptions import ToolRefusedError
from roomkit.tools.compose import extract_tools
from roomkit_tools import AccessRefusedError, InvalidInputError, LimitExceededError, Ocr, OcrTool
from roomkit_tools_ocr_rusto import RustoOcr

ROOT = Path(os.environ["ROOMKIT_OCR_SOURCE"])
FIXTURES = ROOT / "tests/fixtures/ocr"
MODELS = os.getenv("ROOMKIT_OCR_MODELS")


class BackendTests(unittest.IsolatedAsyncioTestCase):
    async def test_lazy_models_typed_validation_and_cancel(self):
        with tempfile.TemporaryDirectory() as models:
            backend = RustoOcr(models)
            tool = await OcrTool.create(FIXTURES, backend=backend)
            # These failures precede loading, even with an empty models directory.
            with self.assertRaises(InvalidInputError):
                await tool.ocr("page.png", rotation_degrees=45)
            with self.assertRaises(InvalidInputError):
                await tool.call({"path": "page.png", "unknown": True})
            with self.assertRaises(InvalidInputError):
                await OcrTool.create(FIXTURES, backend=backend, max_blocks=0)
            with self.assertRaises(InvalidInputError):
                await OcrTool.create(FIXTURES, backend=backend, unknown=1)
            with self.assertRaises(AccessRefusedError):
                await tool.ocr(ROOT / "Cargo.toml")
            with self.assertRaises(AccessRefusedError):
                await tool.ocr("page.png", expected_revision="sha256:" + "0" * 64)
            with self.assertRaises(LimitExceededError):
                limited = await OcrTool.create(FIXTURES, backend=backend, max_file_bytes=1)
                await limited.ocr("page.png")
            task = asyncio.create_task(tool.ocr("page.png"))
            task.cancel()
            with self.assertRaises(asyncio.CancelledError):
                await task
            definitions, handler = extract_tools([Ocr(FIXTURES, backend=backend)])
            self.assertEqual([d.name for d in definitions], ["ocr"])
            with self.assertRaises(ToolRefusedError):
                await handler("ocr", {"path": "page.png", "rotation_degrees": 45})
        with self.assertRaises(InvalidInputError):
            RustoOcr(".", threads=0)

    @unittest.skipUnless(MODELS, "set ROOMKIT_OCR_MODELS for trusted local PP-OCRv6 models")
    async def test_real_models_rust_parity_shared_engine_and_roomkit(self):
        backend = RustoOcr(MODELS)
        tool = await OcrTool.create(FIXTURES, backend=backend)
        original = await asyncio.to_thread((FIXTURES / "page.png").read_bytes)
        result = await tool.ocr("page.png")
        data = result["data"]
        self.assertIn("Free shipping for orders of 50 units or more.", data["text"])
        self.assertEqual(data["revision"], "sha256:" + hashlib.sha256(original).hexdigest())
        self.assertEqual(data["backend"]["provider"], "rusto")
        self.assertTrue(data["backend"]["revision"].startswith("sha256:"))
        self.assertFalse(data["truncated"])
        self.assertEqual(len(data["blocks"]), 5)
        reference = os.getenv("ROOMKIT_OCR_REFERENCE")
        if reference:
            observed = json.loads(await asyncio.to_thread(Path(reference).read_text))
            self.assertEqual(result, observed)
        # Independent bindings use the same engine, preserving its one-worker slot.
        limited = await OcrTool.create(FIXTURES, backend=backend, max_blocks=1)
        full, small = await asyncio.gather(tool.ocr("page.png"), limited.ocr("page.png"))
        self.assertEqual(full, result)
        self.assertEqual(len(small["data"]["blocks"]), 1)
        self.assertEqual(small["data"]["omitted_blocks"], 4)
        self.assertTrue(small["data"]["truncated"])
        _, handler = extract_tools([Ocr(FIXTURES, backend=backend)])
        self.assertEqual(json.loads(await handler("ocr", {"path": "page.png"})), result)
        self.assertEqual(await asyncio.to_thread((FIXTURES / "page.png").read_bytes), original)


@unittest.skipUnless(MODELS, "set ROOMKIT_OCR_MODELS for native shutdown checks")
class ShutdownTests(unittest.TestCase):
    def test_cancelled_native_call_and_both_bridges_can_exit(self):
        program = """
import asyncio, os
from roomkit_tools import OcrTool, ReadTool
from roomkit_tools_ocr_rusto import RustoOcr
async def main():
    read = await ReadTool.create(os.environ["ROOMKIT_OCR_SOURCE"])
    await read.read("Cargo.toml", limit=1)
    tool = await OcrTool.create(
        os.path.join(os.environ["ROOMKIT_OCR_SOURCE"], "tests/fixtures/ocr"),
        backend=RustoOcr(os.environ["ROOMKIT_OCR_MODELS"]),
    )
    task = asyncio.create_task(tool.ocr("page.png"))
    await asyncio.sleep(0.01)
    task.cancel()
    try:
        await task
    except asyncio.CancelledError:
        pass
    else:
        raise AssertionError("expected cancellation")
asyncio.run(main())
"""
        for _ in range(3):
            result = subprocess.run(
                [sys.executable, "-X", "faulthandler", "-I", "-c", program],
                capture_output=True,
                text=True,
                timeout=20,
                check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
