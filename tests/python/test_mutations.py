"""Installed mutation bindings, shared scopes and the Rust commit contract."""

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
    Edit,
    EditTool,
    InvalidInputError,
    LimitExceededError,
    ReadTool,
    ToolIOError,
    UnsupportedResourceError,
    Workspace,
    Write,
    WriteTool,
    _native,
)


def revision(content: bytes) -> str:
    return "sha256:" + hashlib.sha256(content).hexdigest()


class MutationTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self) -> None:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.workspace = Workspace(self.root)
        self.write = await WriteTool.create(self.workspace)
        self.edit = await EditTool.create(self.workspace)
        self.read = await ReadTool.create(self.workspace)

    def test_import_configuration_and_schemas_do_not_require_roomkit(self) -> None:
        self.assertIsNone(importlib.util.find_spec("roomkit"))
        workspace = Workspace(self.root / "later")
        self.assertEqual(os.fspath(workspace), str(self.root / "later"))
        self.assertFalse(Path(workspace).exists())
        for facade, standalone, native, name in [
            (Write, WriteTool, _native._WriteTool, "write"),
            (Edit, EditTool, _native._EditTool, "edit"),
        ]:
            definition = standalone.definition()
            self.assertEqual(definition, json.loads(native.definition_json()))
            self.assertEqual(definition["name"], name)
            self.assertFalse(definition["input_schema"]["additionalProperties"])
            self.assertIn("expected_revision", definition["input_schema"]["properties"])
            self.assertIn("revision", definition["output_schema"]["properties"])
            self.assertEqual(facade(workspace).definition["parameters"], definition["input_schema"])
            definition["name"] = "changed"
            self.assertEqual(standalone.definition()["name"], name)
        with self.assertRaises(InvalidInputError):
            Workspace("")

    async def test_create_edit_remove_and_read_with_revisions(self) -> None:
        first = "Order 43 - Alex: 2 items at 10:00\n"
        second = "Order 44 - Sam: 1 item at 11:00\n"
        written = await self.write.write(Path("orders.txt"), first)
        self.assertEqual(
            written["data"],
            {
                "path": "orders.txt",
                "created": True,
                "changed": True,
                "bytes_written": len(first.encode()),
                "revision": revision(first.encode()),
            },
        )
        added = await self.edit.edit(
            "orders.txt", first, first + second, expected_revision=written["data"]["revision"]
        )
        changed = await self.edit.edit(
            "orders.txt", "2 items", "3 items", expected_revision=added["data"]["revision"]
        )
        removed = await self.edit.edit(
            "orders.txt", second, "", expected_revision=changed["data"]["revision"]
        )
        result = await self.read.read("orders.txt")
        self.assertEqual(result["text"], "1: Order 43 - Alex: 3 items at 10:00\n")
        self.assertEqual(result["data"]["revision"], removed["data"]["revision"])
        self.assertEqual(removed["data"]["replacements"], 1)
        self.assertTrue((self.root / "orders.txt").is_file())

    async def test_overwrite_requires_explicit_flag_even_with_a_revision(self) -> None:
        result = await self.write.write("file.txt", "first")
        rev = result["data"]["revision"]
        for options in [{}, {"expected_revision": rev}]:
            with self.assertRaises(AccessRefusedError):
                await self.write.write("file.txt", "second", **options)
            self.assertEqual((self.root / "file.txt").read_text(), "first")
        overwritten = await self.write.write(
            "file.txt", "second", overwrite=True, expected_revision=rev
        )
        self.assertFalse(overwritten["data"]["created"])
        self.assertTrue(overwritten["data"]["changed"])
        self.assertEqual(overwritten["data"]["revision"], revision(b"second"))

    async def test_stale_revisions_refuse_mutations_even_when_text_still_matches(self) -> None:
        await self.write.write("file.txt", "token: A\nother: X\n")
        old = (await self.read.read("file.txt"))["data"]["revision"]
        current = b"token: A\nother: Y\n"
        (self.root / "file.txt").write_bytes(current)
        for operation in [
            self.write.write("file.txt", "replacement", overwrite=True, expected_revision=old),
            self.edit.edit("file.txt", "A", "B", expected_revision=old),
        ]:
            with self.assertRaises(AccessRefusedError):
                await operation
            self.assertEqual((self.root / "file.txt").read_bytes(), current)
        with self.assertRaises(AccessRefusedError):
            await self.write.write("missing.txt", "value", expected_revision=old)
        self.assertFalse((self.root / "missing.txt").exists())
        with self.assertRaises(InvalidInputError):
            await self.edit.edit("file.txt", "A", "B", expected_revision="not-a-revision")

    async def test_exact_matching_ambiguity_and_empty_replacement(self) -> None:
        await self.write.write("file.txt", "same same\n")
        for old in ["", "absent", "same", "Same"]:
            with self.assertRaises(InvalidInputError):
                await self.edit.edit("file.txt", old, "new")
        self.assertEqual((self.root / "file.txt").read_bytes(), b"same same\n")
        result = await self.edit.edit("file.txt", "same", "", replace_all=True)
        self.assertEqual(result["data"]["replacements"], 2)
        self.assertEqual((self.root / "file.txt").read_bytes(), b" \n")
        with self.assertRaises(InvalidInputError):
            await self.edit.edit("file.txt", "absent", "new", replace_all=True)

    async def test_raw_bytes_revision_and_no_op_are_preserved(self) -> None:
        text = "\ufefffirst π\r\nsecond\r\n"
        written = await self.write.write("file.txt", text)
        raw = text.encode()
        self.assertEqual((self.root / "file.txt").read_bytes(), raw)
        self.assertEqual(written["data"]["revision"], revision(raw))
        read = await self.read.read("file.txt")
        self.assertEqual(read["data"]["revision"], revision(raw))
        edited = await self.edit.edit("file.txt", "second", "last", expected_revision=revision(raw))
        expected = "\ufefffirst π\r\nlast\r\n".encode()
        self.assertEqual((self.root / "file.txt").read_bytes(), expected)
        self.assertEqual(edited["data"]["bytes_before"], len(raw))
        self.assertEqual(edited["data"]["bytes_after"], len(expected))
        before = (self.root / "file.txt").stat()
        for result in [
            await self.write.write("file.txt", expected.decode(), overwrite=True),
            await self.edit.edit("file.txt", "last", "last"),
        ]:
            self.assertFalse(result["data"]["changed"])
            self.assertEqual(result["data"]["revision"], revision(expected))
        after = (self.root / "file.txt").stat()
        self.assertEqual((before.st_ino, before.st_mtime_ns), (after.st_ino, after.st_mtime_ns))
        emptied = await self.write.write("file.txt", "", overwrite=True)
        self.assertEqual(emptied["data"]["bytes_written"], 0)
        self.assertEqual((self.root / "file.txt").read_bytes(), b"")

    async def test_input_limits_and_binary_files_fail_without_side_effects(self) -> None:
        for cls in [WriteTool, EditTool]:
            for value in [0, -1, True, "4", 2**64]:
                with (
                    self.subTest(tool=cls.__name__, bound=value),
                    self.assertRaises(InvalidInputError),
                ):
                    await cls.create(self.workspace, max_file_bytes=value)
        write = await WriteTool.create(self.workspace, max_file_bytes=4)
        edit = await EditTool.create(self.workspace, max_file_bytes=4)
        with self.assertRaises(LimitExceededError):
            await write.write("new.txt", "πππ")
        self.assertFalse((self.root / "new.txt").exists())
        await write.write("file.txt", "aaaa")
        with self.assertRaises(LimitExceededError):
            await edit.edit("file.txt", "a", "bb", replace_all=True)
        self.assertEqual((self.root / "file.txt").read_bytes(), b"aaaa")
        (self.root / "file.txt").write_bytes(b"oversized")
        with self.assertRaises(LimitExceededError):
            await write.write("file.txt", "ok", overwrite=True)
        with self.assertRaises(LimitExceededError):
            await edit.edit("file.txt", "o", "a")
        for raw in [b"bad\0", b"\xff"]:
            (self.root / "file.txt").write_bytes(raw)
            with self.assertRaises(UnsupportedResourceError):
                await self.write.write("file.txt", "new", overwrite=True)
            self.assertEqual((self.root / "file.txt").read_bytes(), raw)
            with self.assertRaises(UnsupportedResourceError):
                await self.edit.edit("file.txt", "bad", "good")
        with self.assertRaises(UnsupportedResourceError):
            await self.write.write("new.txt", "bad\0")

    async def test_rust_input_types_reject_invalid_shapes(self) -> None:
        for tool, base, invalid_values in [
            (
                self.write,
                {"path": "file.txt", "content": "value"},
                [{"content": 1}, {"overwrite": "yes"}],
            ),
            (
                self.edit,
                {"path": "file.txt", "old_text": "x", "new_text": "y"},
                [{"old_text": False}, {"replace_all": 1}],
            ),
        ]:
            for invalid in invalid_values + [
                {"path": 3},
                {"expected_revision": 1},
                {"unknown": True},
            ]:
                with self.assertRaises(InvalidInputError):
                    await tool.call({**base, **invalid})
            with self.assertRaises(InvalidInputError):
                await tool.call({})
        self.assertFalse((self.root / "file.txt").exists())

    async def test_paths_parent_requirements_and_leaf_symlinks(self) -> None:
        child = self.root / "child"
        child.mkdir()
        workspace = Workspace(child)
        write = await WriteTool.create(workspace)
        edit = await EditTool.create(workspace)
        (self.root / "outside.txt").write_text("outside")
        for path in ["../outside.txt", str(self.root / "outside.txt")]:
            with self.assertRaises(AccessRefusedError):
                await write.write(path, "changed", overwrite=True)
            with self.assertRaises(AccessRefusedError):
                await edit.edit(path, "outside", "changed")
        with self.assertRaises(ToolIOError):
            await write.write("absent/file.txt", "value")
        self.assertFalse((child / "absent").exists())
        with self.assertRaises(ToolIOError) as caught:
            await edit.edit("private-missing-file", "a", "b")
        self.assertNotIn("private-missing-file", str(caught.exception))
        if os.name == "posix":
            await write.write("file.txt", "value")
            (child / "link.txt").symlink_to(child / "file.txt")
            with self.assertRaises(AccessRefusedError):
                await write.write("link.txt", "new", overwrite=True)
            with self.assertRaises(AccessRefusedError):
                await edit.edit("link.txt", "value", "new")
            self.assertEqual((child / "file.txt").read_text(), "value")

    async def test_concurrent_write_and_edit_share_revision_check_and_commit(self) -> None:
        workspace = Workspace(self.root)
        write, edit = await asyncio.gather(WriteTool.create(workspace), EditTool.create(workspace))
        old = (await write.write("file.txt", "original\n"))["data"]["revision"]
        results = await asyncio.gather(
            write.write("file.txt", "written\n", overwrite=True, expected_revision=old),
            edit.edit("file.txt", "original", "edited", expected_revision=old),
            return_exceptions=True,
        )
        self.assertEqual(sum(isinstance(result, dict) for result in results), 1)
        self.assertEqual(sum(isinstance(result, AccessRefusedError) for result in results), 1)
        self.assertIn((self.root / "file.txt").read_text(), {"written\n", "edited\n"})

    async def test_concurrent_edits_do_not_lose_unrelated_updates(self) -> None:
        workspace = Workspace(self.root)
        edits = await asyncio.gather(*(EditTool.create(workspace) for _ in range(8)))
        (self.root / "file.txt").write_text("".join(f"slot{i}=old\n" for i in range(8)))
        await asyncio.gather(
            *(tool.edit("file.txt", f"slot{i}=old", f"slot{i}=new") for i, tool in enumerate(edits))
        )
        self.assertEqual(
            (self.root / "file.txt").read_text(), "".join(f"slot{i}=new\n" for i in range(8))
        )

    async def test_native_precancellation_has_no_effect_and_allows_retry(self) -> None:
        cancelled = _native._Cancellation()
        cancelled.cancel()
        for native, arguments in [
            (_native._WriteTool, {"path": "file.txt", "content": "value"}),
            (_native._EditTool, {"path": "file.txt", "old_text": "old", "new_text": "new"}),
        ]:
            with self.assertRaises(asyncio.CancelledError):
                await native.create(self.workspace._native, "{}", cancelled)
            tool = await native.create(self.workspace._native, "{}", _native._Cancellation())
            with self.assertRaises(asyncio.CancelledError):
                await tool.call(json.dumps(arguments), cancelled)
        self.assertFalse((self.root / "file.txt").exists())
        await self.write.write("file.txt", "retry")
        self.assertEqual((self.root / "file.txt").read_text(), "retry")

    async def test_python_cancellation_preserves_whole_file_and_settles_worker(self) -> None:
        before = b"old\n" + b"line\n" * (4 * 1024 * 1024)
        write = await WriteTool.create(self.workspace, max_file_bytes=32 * 1024 * 1024)
        edit = await EditTool.create(self.workspace, max_file_bytes=32 * 1024 * 1024)
        for tool, arguments, after in [
            (write, {"path": "large.txt", "content": "new\n", "overwrite": True}, b"new\n"),
            (
                edit,
                {"path": "large.txt", "old_text": "old", "new_text": "new"},
                b"new" + before[3:],
            ),
        ]:
            (self.root / "large.txt").write_bytes(before)
            cancel = _native._Cancellation()
            with patch("roomkit_tools.fs._native._Cancellation", return_value=cancel):
                task = asyncio.create_task(tool.call(arguments))
                await asyncio.sleep(0)
                self.assertFalse(task.done())
                task.cancel()
                with self.assertRaises(asyncio.CancelledError):
                    await task
            self.assertTrue(cancel.is_cancelled())
            # A subsequent operation on the same scope waits for any worker
            # whose commit had already started. Never assume cancellation rolls back.
            await write.write("barrier.txt", "done", overwrite=True)
            self.assertIn((self.root / "large.txt").read_bytes(), [before, after])
            self.assertEqual(list(self.root.glob(".roomkit-write-*")), [])

    async def test_mutation_io_does_not_block_the_python_event_loop(self) -> None:
        (self.root / "large.txt").write_bytes(b"old\n" + b"line\n" * (6 * 1024 * 1024))
        edit = await EditTool.create(self.workspace, max_file_bytes=32 * 1024 * 1024)
        task = asyncio.create_task(edit.edit("large.txt", "old", "new"))
        ticks = 0
        async with asyncio.timeout(15):
            while not task.done():
                await asyncio.sleep(0.001)
                ticks += 1
        self.assertGreater(ticks, 1)
        self.assertTrue((await task)["data"]["changed"])


class WorkspaceLifecycleTests(unittest.TestCase):
    def test_relative_root_retry_and_reuse_across_event_loops(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            cwd = Path.cwd()
            try:
                os.chdir(directory)
                workspace = Workspace("later")
            finally:
                os.chdir(cwd)
            self.assertEqual(os.fspath(workspace), str(Path(directory) / "later"))
            with self.assertRaises(ToolIOError):
                asyncio.run(WriteTool.create(workspace))
            Path(workspace).mkdir()
            write = asyncio.run(WriteTool.create(workspace))
            edit = asyncio.run(EditTool.create(workspace))
            first = asyncio.run(write.write("file.txt", "old"))
            result = asyncio.run(
                edit.edit("file.txt", "old", "new", expected_revision=first["data"]["revision"])
            )
            self.assertEqual(result["data"]["revision"], revision(b"new"))
            self.assertEqual(Path(workspace, "file.txt").read_text(), "new")


if __name__ == "__main__":
    unittest.main()
