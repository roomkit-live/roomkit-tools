"""Behavior at the installed Python wheel's Rust, JSON and asyncio boundaries."""

from __future__ import annotations

import asyncio
import hashlib
import importlib.util
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from roomkit_tools import (
    AccessRefusedError,
    InvalidInputError,
    LimitExceededError,
    ReadTool,
    ToolError,
    ToolIOError,
    UnsupportedResourceError,
    _native,
)


class ReadTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self) -> None:
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.source = self.root / "source.txt"
        self.source.write_bytes("\ufefffirst π\r\nsecond\r\nlast".encode())
        self.tool = await ReadTool.create(self.root)

    def test_native_extension_is_installed_and_core_import_needs_no_roomkit(self) -> None:
        self.assertIsNone(importlib.util.find_spec("roomkit"))
        self.assertIn("site-packages", _native.__file__)
        self.assertTrue(_native.__file__.endswith((".so", ".pyd")))

    async def test_pagination_preserves_structured_results_and_rereads_changes(self) -> None:
        result = await self.tool.read("source.txt", limit=1)
        self.assertEqual(
            result,
            {
                "data": {
                    "path": str(self.source.resolve()),
                    "lines": [{"number": 1, "text": "first π"}],
                    "truncated": True,
                    "next_offset": 2,
                    "revision": "sha256:" + hashlib.sha256(self.source.read_bytes()).hexdigest(),
                },
                "text": "1: first π\n",
            },
        )
        last = await self.tool.read(self.source, offset=result["data"]["next_offset"])
        self.assertEqual(last["text"], "2: second\n3: last\n")
        self.assertIsNone(last["data"]["next_offset"])
        self.assertEqual(last["data"]["revision"], result["data"]["revision"])
        self.assertEqual(self.source.read_bytes(), "\ufefffirst π\r\nsecond\r\nlast".encode())
        self.source.write_text("changed\n")
        changed = await self.tool.read("source.txt")
        self.assertEqual(changed["text"], "1: changed\n")
        self.assertNotEqual(changed["data"]["revision"], result["data"]["revision"])

    def test_definition_is_the_rust_schema_and_each_copy_is_independent(self) -> None:
        definition = self.tool.definition()
        self.assertEqual(definition, json.loads(_native._ReadTool.definition_json()))
        self.assertEqual(definition["name"], "read")
        schema = definition["input_schema"]
        self.assertFalse(schema["additionalProperties"])
        self.assertEqual(schema["properties"]["offset"]["minimum"], 1)
        self.assertIn("next_offset", definition["output_schema"]["properties"])
        definition["name"] = "changed"
        self.assertEqual(self.tool.definition()["name"], "read")

    async def test_actual_rust_input_validation_rejects_invalid_json_shapes(self) -> None:
        for fields in [
            {"offset": 0},
            {"offset": -1},
            {"offset": True},
            {"offset": 1.5},
            {"limit": 0},
            {"unknown": True},
            {"path": 1},
        ]:
            with self.subTest(fields=fields), self.assertRaises(InvalidInputError):
                await self.tool.call({"path": "source.txt", **fields})
        with self.assertRaises(InvalidInputError):
            await self.tool.call({})
        with self.assertRaises(ValueError):
            await self.tool.call({"path": "source.txt", "offset": float("nan")})

    async def test_host_limits_and_page_bounds_are_enforced(self) -> None:
        small = await ReadTool.create(self.root, max_lines=1)
        self.assertEqual(len((await small.read("source.txt", limit=100))["data"]["lines"]), 1)
        for options in [{"max_lines": 0}, {"max_file_bytes": -1}, {"max_lines": True}]:
            with self.subTest(options=options), self.assertRaises(InvalidInputError):
                await ReadTool.create(self.root, **options)
        for options in [{"max_file_bytes": 1}, {"max_output_bytes": 1}]:
            limited = await ReadTool.create(self.root, **options)
            with self.subTest(options=options), self.assertRaises(LimitExceededError):
                await limited.read("source.txt")
        with self.assertRaises(InvalidInputError):
            await self.tool.read("source.txt", offset=99)

    async def test_empty_and_binary_files_are_distinct(self) -> None:
        self.source.write_bytes(b"")
        result = await self.tool.read("source.txt")
        self.assertEqual(result["data"]["lines"], [])
        self.assertFalse(result["data"]["truncated"])
        for content in [b"data\0", b"\xff"]:
            self.source.write_bytes(content)
            with self.assertRaises(UnsupportedResourceError):
                await self.tool.read("source.txt")
        with self.assertRaises(UnsupportedResourceError):
            await self.tool.read(".")

    async def test_roots_do_not_change_cwd_and_outside_paths_are_refused(self) -> None:
        cwd = Path.cwd()
        child = self.root / "child"
        child.mkdir()
        other = await ReadTool.create(child)
        for path in ["../source.txt", str(self.source)]:
            with self.subTest(path=path), self.assertRaises(AccessRefusedError):
                await other.read(path)
        self.assertEqual(Path.cwd(), cwd)
        if os.name == "posix":
            (child / "link").symlink_to(self.source)
            with self.assertRaises(AccessRefusedError):
                await other.read("link")

    async def test_errors_have_safe_messages_and_a_common_base(self) -> None:
        with self.assertRaises(ToolIOError) as caught:
            await self.tool.read("missing-private-name.txt")
        self.assertIsInstance(caught.exception, ToolError)
        self.assertNotIn("missing-private-name", str(caught.exception))
        self.assertNotIn(str(self.root), str(caught.exception))
        with self.assertRaises(InvalidInputError):
            await ReadTool.create(self.source)
        with self.assertRaises(ToolIOError):
            await ReadTool.create(self.root / "missing")

    async def test_rust_cancellation_maps_to_asyncio_cancelled_error(self) -> None:
        cancel = _native._Cancellation()
        cancel.cancel()
        native = self.tool._native
        with self.assertRaises(asyncio.CancelledError):
            await native.read('{"path":"source.txt"}', cancel)
        self.assertEqual(
            (await self.tool.read("source.txt"))["data"]["lines"][0]["text"], "first π"
        )

    async def test_task_cancellation_reaches_the_token_used_by_rust(self) -> None:
        large = self.root / "large.txt"
        large.write_bytes(b"line\n" * (6 * 1024 * 1024))
        tool = await ReadTool.create(self.root, max_file_bytes=32 * 1024 * 1024, max_lines=1)
        cancel = _native._Cancellation()
        with patch("roomkit_tools.fs._native._Cancellation", return_value=cancel):
            task = asyncio.create_task(tool.read("large.txt"))
            await asyncio.sleep(0)
            self.assertFalse(task.done())
            task.cancel()
            with self.assertRaises(asyncio.CancelledError):
                await task
        self.assertTrue(cancel.is_cancelled())
        self.assertEqual(
            (await self.tool.read("source.txt"))["text"], "1: first π\n2: second\n3: last\n"
        )

    async def test_file_io_keeps_the_python_event_loop_responsive(self) -> None:
        large = self.root / "large.txt"
        large.write_bytes(b"line\n" * (6 * 1024 * 1024))
        tool = await ReadTool.create(self.root, max_file_bytes=32 * 1024 * 1024, max_lines=1)
        task = asyncio.create_task(tool.read("large.txt"))
        ticks = 0
        async with asyncio.timeout(10):
            while not task.done():
                await asyncio.sleep(0.001)
                ticks += 1
        self.assertGreater(ticks, 1)
        self.assertEqual((await task)["text"], "1: line\n")


class EventLoopTests(unittest.TestCase):
    def test_a_tool_can_be_reused_after_its_original_event_loop_closes(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            Path(directory, "source.txt").write_text("same Rust tool\n")
            tool = asyncio.run(ReadTool.create(directory))
            first = asyncio.run(tool.read("source.txt"))
            second = asyncio.run(tool.read("source.txt"))
            self.assertEqual(first, second)
            self.assertEqual(second["text"], "1: same Rust tool\n")


if __name__ == "__main__":
    unittest.main()
