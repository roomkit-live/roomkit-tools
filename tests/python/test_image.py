"""Installed image preparation: Rust parity, actual PNG payloads and boundaries."""

import asyncio
import base64
import hashlib
import json
import os
import struct
import tempfile
import unittest
from contextlib import nullcontext
from pathlib import Path
from unittest.mock import patch

from roomkit_tools import AccessRefusedError, InvalidInputError, ViewImageTool, _native
from test_contracts import ERRORS, normalize, pointer

FIXTURE = json.loads((Path(__file__).resolve().parents[1] / "fixtures/image.json").read_text())


class ImageTests(unittest.IsolatedAsyncioTestCase):
    async def test_complete_rust_python_envelopes(self):
        reference_path = os.getenv("ROOMKIT_IMAGE_RESULTS")
        reference = (
            json.loads(await asyncio.to_thread(Path(reference_path).read_text))
            if reference_path
            else None
        )
        if reference:
            self.assertEqual(reference["fixture"], FIXTURE)
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for name, data in FIXTURE["files"].items():
                (root / name).write_bytes(base64.b64decode(data))
            for case in FIXTURE["cases"]:
                with self.subTest(case=case["name"]):
                    tool = await ViewImageTool.create(root, **case.get("limits", {}))
                    token = _native._Cancellation()
                    token.cancel()
                    context = (
                        patch.object(_native, "_Cancellation", return_value=token)
                        if case.get("cancelled")
                        else nullcontext()
                    )
                    try:
                        with context:
                            result = await tool.call(case["input"])
                    except tuple(ERRORS) as error:
                        result = {"error": ERRORS[type(error)]}
                    result = normalize(result, root)
                    for key, expected in case["expect"].items():
                        self.assertEqual(pointer(result, key), expected)
                    if reference:
                        self.assertEqual(result, reference["reports"][case["name"]])

    async def test_native_payload_is_png_source_is_unchanged_and_not_in_text(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            original = base64.b64decode(FIXTURE["files"]["colors.png"])
            (root / "colors.png").write_bytes(original)
            tool = await ViewImageTool.create(root)
            result = await tool.view("colors.png")
            data = result["data"]
            pixels = base64.b64decode(data["image"]["base64"], validate=True)
            self.assertTrue(pixels.startswith(b"\x89PNG\r\n\x1a\n"))
            self.assertEqual(struct.unpack(">II", pixels[16:24]), (data["width"], data["height"]))
            self.assertEqual(len(pixels), data["output_bytes"])
            self.assertEqual(data["revision"], "sha256:" + hashlib.sha256(original).hexdigest())
            self.assertEqual((root / "colors.png").read_bytes(), original)
            self.assertNotIn(data["image"]["base64"], result["text"])
            self.assertEqual(ViewImageTool.definition()["name"], "view_image")

    async def test_a_timeout_above_the_maximum_is_refused_with_it(self):
        with tempfile.TemporaryDirectory() as root:
            pixels = base64.b64decode(FIXTURE["files"]["colors.png"])
            await asyncio.to_thread(Path(root, "colors.png").write_bytes, pixels)
            tool = await ViewImageTool.create(root, default_timeout_ms=1000, max_timeout_ms=1000)
            with self.assertRaisesRegex(
                InvalidInputError, "timeout_ms exceeds the host maximum of 1000 ms"
            ):
                await tool.view("colors.png", timeout_ms=1001)
            result = await tool.view("colors.png", timeout_ms=1000)
            self.assertEqual(result["data"]["width"], 2)

    async def test_invalid_limits_root_and_cancellation_are_explicit(self):
        with tempfile.TemporaryDirectory() as root, tempfile.TemporaryDirectory() as outside:
            path = Path(outside, "image.png")
            await asyncio.to_thread(
                path.write_bytes, base64.b64decode(FIXTURE["files"]["colors.png"])
            )
            with self.assertRaises(InvalidInputError):
                await ViewImageTool.create(path)
            with self.assertRaises(InvalidInputError):
                await ViewImageTool.create(root, max_pixels=0)
            tool = await ViewImageTool.create(root)
            with self.assertRaises(AccessRefusedError):
                await tool.view(path)
            task = asyncio.create_task(tool.view("missing.png"))
            task.cancel()
            with self.assertRaises(asyncio.CancelledError):
                await task
