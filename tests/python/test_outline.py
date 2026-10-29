"""Installed source outlines: full Rust envelope parity and file boundaries."""

import asyncio
import json
import os
import tempfile
import unittest
from contextlib import nullcontext
from pathlib import Path
from unittest.mock import patch

from roomkit_tools import AccessRefusedError, CodeOutlineTool, InvalidInputError, ReadTool, _native
from test_contracts import ERRORS, normalize, pointer

FIXTURE = json.loads((Path(__file__).resolve().parents[1] / "fixtures/outline.json").read_text())


class OutlineTests(unittest.IsolatedAsyncioTestCase):
    async def test_shared_rust_python_cases_and_complete_envelopes(self):
        reference_path = os.getenv("ROOMKIT_OUTLINE_RESULTS")
        reference = (
            json.loads(await asyncio.to_thread(Path(reference_path).read_text))
            if reference_path
            else None
        )
        if reference:
            self.assertEqual(reference["fixture"], FIXTURE)
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for path, text in FIXTURE["files"].items():
                (root / path).write_bytes(text.encode())
            for case in FIXTURE["cases"]:
                with self.subTest(case=case["name"]):
                    tool = await CodeOutlineTool.create(root, **case.get("limits", {}))
                    token = _native._Cancellation()
                    token.cancel()
                    scope = (
                        patch.object(_native, "_Cancellation", return_value=token)
                        if case.get("cancelled")
                        else nullcontext()
                    )
                    try:
                        with scope:
                            result = await tool.call(case["input"])
                    except tuple(ERRORS) as error:
                        result = {"error": ERRORS[type(error)]}
                    result = normalize(result, root)
                    for key, expected in case["expect"].items():
                        self.assertEqual(pointer(result, key), expected)
                    if reference:
                        self.assertEqual(result, reference["reports"][case["name"]])

    async def test_revision_matches_read_and_refuses_changed_pagination(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            path = root / "source.py"
            path.write_bytes(FIXTURE["files"]["unicode.py"].encode())
            tool = await CodeOutlineTool.create(root)
            page = (await tool.outline(path, limit=1))["data"]
            read = await ReadTool.create(root)
            self.assertEqual(page["revision"], (await read.read(path))["data"]["revision"])
            following = await tool.outline(
                path, offset=page["next_offset"], expected_revision=page["revision"]
            )
            self.assertEqual(following["data"]["symbols"][0]["parent"], 1)
            path.write_text("def changed(): pass\n")
            with self.assertRaises(AccessRefusedError):
                await tool.outline(path, expected_revision=page["revision"])

    async def test_real_schema_and_configuration_are_native(self):
        definition = CodeOutlineTool.definition()
        self.assertEqual(definition, json.loads(_native._CodeOutlineTool.definition_json()))
        self.assertEqual(definition["name"], "code_outline")
        self.assertFalse(definition["input_schema"]["additionalProperties"])
        with tempfile.TemporaryDirectory() as root:
            with self.assertRaises(InvalidInputError):
                await CodeOutlineTool.create(root, max_depth=0)
            tool = await CodeOutlineTool.create(root)
            with self.assertRaises(AccessRefusedError):
                await tool.outline(__file__, language="python")


class OutlineLoopTests(unittest.TestCase):
    def test_outline_is_reusable_across_event_loops(self):
        with tempfile.TemporaryDirectory() as root:
            Path(root, "source.rs").write_text("fn example() {}\n")
            tool = asyncio.run(CodeOutlineTool.create(root))
            one = asyncio.run(tool.outline("source.rs"))
            two = asyncio.run(tool.outline("source.rs"))
            self.assertEqual(one, two)
