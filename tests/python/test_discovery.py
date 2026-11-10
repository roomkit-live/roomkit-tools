"""Installed discovery bindings: actual Rust behavior and asyncio boundaries."""

from __future__ import annotations

import asyncio
import importlib.util
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from roomkit_tools import (
    AccessRefusedError,
    Glob,
    GlobTool,
    Grep,
    GrepTool,
    InvalidInputError,
    LimitExceededError,
    List,
    ListTool,
    ReadTool,
    ToolIOError,
    UnsupportedResourceError,
    _native,
)


class DiscoveryTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self) -> None:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        (self.root / "catalog").mkdir()
        (self.root / "catalog/items.txt").write_text(
            "Widget A\nPrice: 12 EUR\nTags: blue, small\n\n"
            "Widget B\nPrice: 20 EUR\nTags: RED, large\n"
        )
        (self.root / "catalog/notes.md").write_text("Blue plan.\nblueprint\n")
        (self.root / "notes.txt").write_text("literal [blue]\n")
        (self.root / ".hidden.txt").write_text("hidden\n")
        (self.root / ".gitignore").write_text("catalog/\n")
        (self.root / "binary.dat").write_bytes(b"blue\0")

    async def test_schemas_and_adapters_need_no_roomkit(self) -> None:
        self.assertIsNone(importlib.util.find_spec("roomkit"))
        for wrapper, facade, native, name in [
            (ListTool, List, _native._ListTool, "list"),
            (GlobTool, Glob, _native._GlobTool, "glob"),
            (GrepTool, Grep, _native._GrepTool, "grep"),
        ]:
            with self.subTest(name=name):
                definition = wrapper.definition()
                self.assertEqual(definition, json.loads(native.definition_json()))
                self.assertEqual(definition["name"], name)
                self.assertFalse(definition["input_schema"]["additionalProperties"])
                self.assertEqual(definition["input_schema"]["properties"]["offset"]["minimum"], 1)
                configured = facade(await wrapper.create(self.root))
                self.assertEqual(configured.definition["parameters"], definition["input_schema"])
                definition["name"] = "changed"
                self.assertEqual(configured.definition["name"], name)
                with self.assertRaises(TypeError):
                    facade(self.root)

    async def test_list_sorted_pages_include_hidden_entries_and_types(self) -> None:
        tool = await ListTool.create(self.root, max_entries=2)
        entries = []
        offset = 1
        while offset is not None:
            page = (await tool.list(limit=100, offset=offset))["data"]
            self.assertEqual(page["path"], ".")
            self.assertLessEqual(len(page["entries"]), 2)
            self.assertEqual(page["truncated"], page["next_offset"] is not None)
            entries.extend(page["entries"])
            offset = page["next_offset"]
        self.assertEqual(
            [entry["path"] for entry in entries],
            [".gitignore", ".hidden.txt", "binary.dat", "catalog", "notes.txt"],
        )
        self.assertEqual(entries[3], {"kind": "directory", "name": "catalog", "path": "catalog"})
        nested = await tool.list(Path("catalog"))
        self.assertEqual(nested["data"]["entries"][0]["path"], "catalog/items.txt")
        (self.root / "empty").mkdir()
        self.assertEqual((await tool.list("empty"))["data"]["entries"], [])
        with self.assertRaises(InvalidInputError):
            await tool.list(offset=999)

    async def test_glob_patterns_pagination_and_paths_can_be_read(self) -> None:
        tool = await GlobTool.create(self.root, max_matches=1)
        entries = []
        offset = 1
        while offset is not None:
            result = await tool.glob("**/*.txt", offset=offset)
            entries.extend(result["data"]["entries"])
            offset = result["data"]["next_offset"]
        self.assertEqual(
            [entry["path"] for entry in entries],
            [".hidden.txt", "catalog/items.txt", "notes.txt"],
        )
        nested = await tool.glob("*.txt", path=Path("catalog"))
        path = nested["data"]["entries"][0]["path"]
        read = await ReadTool.create(self.root)
        self.assertTrue((await read.read(path))["text"].startswith("1: Widget A\n"))
        self.assertEqual((await tool.glob("*.absent"))["data"]["entries"], [])

    async def test_grep_context_is_structured_and_offsets_count_matches(self) -> None:
        tool = await GrepTool.create(self.root, max_matches=1)
        first = await tool.grep("Tags:", include="**/*.txt", context_before=2, context_after=1)
        self.assertEqual(
            first["data"]["matches"],
            [
                {
                    "path": "catalog/items.txt",
                    "line": 3,
                    "text": "Tags: blue, small",
                    "context_before": [
                        {"number": 1, "text": "Widget A"},
                        {"number": 2, "text": "Price: 12 EUR"},
                    ],
                    "context_after": [{"number": 4, "text": ""}],
                }
            ],
        )
        self.assertEqual(first["data"]["next_offset"], 2)
        self.assertIn('"catalog/items.txt"-1- Widget A\n', first["text"])
        last = await tool.grep("Tags:", include="**/*.txt", context_before=2, offset=2)
        self.assertEqual(last["data"]["matches"][0]["context_before"][0]["text"], "Widget B")
        self.assertNotIn("context_after", last["data"]["matches"][0])
        self.assertIsNone(last["data"]["next_offset"])

    async def test_grep_regex_literal_case_file_filter_and_binary_skip(self) -> None:
        tool = await GrepTool.create(self.root)
        matches = (await tool.grep(r"\bblue\b", ignore_case=True))["data"]
        self.assertEqual(matches["skipped_files"], 1)
        self.assertEqual(
            [m["path"] for m in matches["matches"]],
            [
                "catalog/items.txt",
                "catalog/notes.md",
                "notes.txt",
            ],
        )
        literal = await tool.grep("[blue]", literal=True, path=Path("notes.txt"), include="*.txt")
        self.assertEqual(literal["data"]["matches"][0]["text"], "literal [blue]")
        plain = await tool.grep("Tags:", path="catalog", include="*.txt")
        self.assertEqual(len(plain["data"]["matches"]), 2)
        # Without "/", an include filters file names at any depth.
        nested = await tool.grep("Tags:", include="*.txt")
        self.assertEqual(
            [match["path"] for match in nested["data"]["matches"]],
            ["catalog/items.txt", "catalog/items.txt"],
        )
        self.assertNotIn("context_before", plain["data"]["matches"][0])
        empty = await tool.grep("missing text")
        self.assertEqual(empty["data"]["matches"], [])
        self.assertFalse(empty["data"]["truncated"])

    async def test_arguments_are_validated_by_rust(self) -> None:
        for cls, arguments in [
            (ListTool, {}),
            (GlobTool, {"pattern": "**"}),
            (GrepTool, {"pattern": "blue"}),
        ]:
            tool = await cls.create(self.root)
            for invalid in [
                {"offset": 0},
                {"offset": True},
                {"limit": 0},
                {"limit": 1.5},
                {"path": 3},
                {"unknown": True},
            ]:
                with (
                    self.subTest(tool=cls.__name__, invalid=invalid),
                    self.assertRaises(InvalidInputError),
                ):
                    await tool.call({**arguments, **invalid})
        for cls in [GlobTool, GrepTool]:
            tool = await cls.create(self.root)
            for arguments in [{}, {"pattern": "["}, {"pattern": False}]:
                with (
                    self.subTest(tool=cls.__name__, args=arguments),
                    self.assertRaises(InvalidInputError),
                ):
                    await tool.call(arguments)
        tool = await GrepTool.create(self.root)
        for invalid in [
            {"literal": "yes"},
            {"ignore_case": 1},
            {"context_before": -1},
            {"context_after": True},
            {"include": 2},
        ]:
            with self.subTest(invalid=invalid), self.assertRaises(InvalidInputError):
                await tool.call({"pattern": "blue", **invalid})

    async def test_host_options_are_validated_and_enforced(self) -> None:
        for cls, arguments, fields in [
            (ListTool, {}, ["max_entries", "max_output_bytes", "max_scan_entries"]),
            (
                GlobTool,
                {"pattern": "**"},
                ["max_matches", "max_output_bytes", "max_scan_entries", "max_depth"],
            ),
            (
                GrepTool,
                {"pattern": "blue"},
                [
                    "max_matches",
                    "max_output_bytes",
                    "max_scan_entries",
                    "max_depth",
                    "max_file_bytes",
                    "max_total_bytes",
                ],
            ),
        ]:
            for field in fields:
                for value in [0, -1, True]:
                    with (
                        self.subTest(tool=cls.__name__, field=field, value=value),
                        self.assertRaises(InvalidInputError),
                    ):
                        await cls.create(self.root, **{field: value})
                if field not in {"max_entries", "max_matches"}:
                    tool = await cls.create(self.root, **{field: 1})
                    with (
                        self.subTest(tool=cls.__name__, field=field),
                        self.assertRaises(LimitExceededError),
                    ):
                        await tool.call(arguments)
        tool = await GrepTool.create(self.root, max_context_lines=0)
        self.assertEqual(len((await tool.grep("Tags:"))["data"]["matches"]), 2)
        for context in [{"context_before": 1}, {"context_after": 1}]:
            with self.assertRaises(LimitExceededError):
                await tool.grep("Tags:", **context)
        for value in [-1, True]:
            with self.assertRaises(InvalidInputError):
                await GrepTool.create(self.root, max_context_lines=value)
        for value in [0, -1, True]:
            with self.assertRaises(InvalidInputError):
                await GrepTool.create(self.root, max_line_bytes=value)
        short = await GrepTool.create(self.root, max_line_bytes=4)
        page = (await short.grep("Tags:"))["data"]
        self.assertEqual(page["matches"], [])
        self.assertGreater(page["skipped_lines"], 0)

    async def test_roots_errors_and_child_symlinks(self) -> None:
        cwd = Path.cwd()
        for cls, arguments in [
            (ListTool, {}),
            (GlobTool, {"pattern": "**"}),
            (GrepTool, {"pattern": "blue"}),
        ]:
            tool = await cls.create(self.root / "catalog")
            for path in ["../notes.txt", str(self.root / "notes.txt")]:
                with (
                    self.subTest(tool=cls.__name__, path=path),
                    self.assertRaises(AccessRefusedError),
                ):
                    await tool.call({**arguments, "path": path})
            with self.assertRaises(ToolIOError) as error:
                await tool.call({**arguments, "path": "missing-private-file"})
            self.assertNotIn("missing-private-file", str(error.exception))
            with self.assertRaises(InvalidInputError):
                await cls.create(self.root / "notes.txt")
            with self.assertRaises(ToolIOError):
                await cls.create(self.root / "missing")
            if cls is not GrepTool:
                with self.assertRaises(UnsupportedResourceError):
                    await tool.call({**arguments, "path": "items.txt"})
        self.assertEqual(Path.cwd(), cwd)
        if os.name == "posix":
            (self.root / "catalog/link").symlink_to(self.root / "notes.txt")
            listing = await ListTool.create(self.root / "catalog")
            result = await listing.list()
            self.assertEqual(result["data"]["entries"][1]["kind"], "symlink")
            grep = await GrepTool.create(self.root / "catalog")
            self.assertEqual((await grep.grep("literal"))["data"]["matches"], [])
            with self.assertRaises(AccessRefusedError):
                await grep.grep("literal", path="link")

    async def test_native_precancellation_and_python_cancellation(self) -> None:
        scan = self.root / "scan"
        scan.mkdir()
        for index in range(500):
            (scan / f"{index:04}.txt").write_text("blue\n")
        for cls, native, arguments in [
            (ListTool, _native._ListTool, {}),
            (GlobTool, _native._GlobTool, {"pattern": "**/*.txt"}),
            (GrepTool, _native._GrepTool, {"pattern": "blue"}),
        ]:
            with self.subTest(tool=cls.__name__):
                tool = await cls.create(self.root)
                cancelled = _native._Cancellation()
                cancelled.cancel()
                with self.assertRaises(asyncio.CancelledError):
                    await native.create(str(self.root), "{}", cancelled)
                with self.assertRaises(asyncio.CancelledError):
                    await tool._native.call(json.dumps(arguments), cancelled)
                cancel = _native._Cancellation()
                with patch("roomkit_tools.fs._native._Cancellation", return_value=cancel):
                    task = asyncio.create_task(tool.call({**arguments, "path": "scan"}))
                    await asyncio.sleep(0)
                    self.assertFalse(task.done())
                    task.cancel()
                    with self.assertRaises(asyncio.CancelledError):
                        await task
                self.assertTrue(cancel.is_cancelled())
                await tool.call({**arguments, "path": "catalog"})

    async def test_grep_does_not_block_the_event_loop(self) -> None:
        (self.root / "large.txt").write_bytes(b"line\n" * (6 * 1024 * 1024))
        tool = await GrepTool.create(self.root, max_file_bytes=32 * 1024 * 1024)
        task = asyncio.create_task(tool.grep("absent", path="large.txt"))
        ticks = 0
        async with asyncio.timeout(15):
            while not task.done():
                await asyncio.sleep(0.001)
                ticks += 1
        self.assertGreater(ticks, 1)
        self.assertEqual((await task)["data"]["matches"], [])


class EventLoopTests(unittest.TestCase):
    def test_discovery_tools_can_be_reused_across_event_loops(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            Path(directory, "source.txt").write_text("blue\n")
            for cls, args in [
                (ListTool, {}),
                (GlobTool, {"pattern": "*.txt"}),
                (GrepTool, {"pattern": "blue"}),
            ]:
                tool = asyncio.run(cls.create(directory))
                self.assertEqual(asyncio.run(tool.call(args)), asyncio.run(tool.call(args)))


if __name__ == "__main__":
    unittest.main()
