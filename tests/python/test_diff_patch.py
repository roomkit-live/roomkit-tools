"""Installed comparison and patch bindings exercise the existing Rust contracts."""

from __future__ import annotations

import asyncio
import hashlib
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from roomkit_tools import (
    AccessRefusedError,
    Diff,
    DiffTool,
    EditTool,
    InvalidInputError,
    LimitExceededError,
    Patch,
    PatchTool,
    ToolIOError,
    UnsupportedResourceError,
    Workspace,
    WriteTool,
    _native,
)


class DiffPatchTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self) -> None:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.workspace = Workspace(self.root)
        self.diff = await DiffTool.create(self.workspace)
        self.patch = await PatchTool.create(self.workspace)

    async def test_adapters_and_rust_schemas_without_roomkit(self) -> None:
        self.assertIsNone(importlib.util.find_spec("roomkit"))
        for facade, cls, native, name in [
            (Diff, DiffTool, _native._DiffTool, "diff"),
            (Patch, PatchTool, _native._PatchTool, "patch"),
        ]:
            definition = cls.definition()
            self.assertEqual(definition, json.loads(native.definition_json()))
            self.assertEqual(definition["name"], name)
            self.assertFalse(definition["input_schema"]["additionalProperties"])
            configured = facade(await cls.create(self.root))
            self.assertEqual(configured.definition["parameters"], definition["input_schema"])
            definition["name"] = "changed"
            self.assertEqual(cls.definition()["name"], name)
        self.assertTrue(PatchTool.definition()["input_schema"]["properties"]["dry_run"]["default"])
        self.assertIn("failure", PatchTool.definition()["output_schema"]["properties"])

    async def test_diff_preserves_exact_lines_and_does_not_mutate(self) -> None:
        old, new = "\ufefftitle\r\nold\r\nlast", "\ufefftitle\r\nnew\nlast"
        (self.root / "old.txt").write_bytes(old.encode())
        (self.root / "new.txt").write_bytes(new.encode())
        result = await self.diff.diff(Path("old.txt"), Path("new.txt"))
        self.assertTrue(result["data"]["changed"])
        self.assertEqual(result["data"]["additions"], 1)
        self.assertEqual(result["data"]["deletions"], 1)
        lines = result["data"]["hunks"][0]["lines"]
        self.assertEqual("".join(x["text"] for x in lines if x["kind"] != "added"), old)
        self.assertEqual("".join(x["text"] for x in lines if x["kind"] != "removed"), new)
        for name, content in [("old.txt", old), ("new.txt", new)]:
            self.assertEqual((self.root / name).read_bytes(), content.encode())
        same = await self.diff.diff("old.txt", "old.txt", context_lines=0)
        self.assertFalse(same["data"]["changed"])
        self.assertEqual(same["data"]["hunks"], [])

    async def test_preview_then_apply_all_four_operations(self) -> None:
        original = {"update": "before\n", "remove": "gone\n", "move": "\ufeffπ\r\n"}
        for name, text in original.items():
            (self.root / name).write_bytes(text.encode())
        changes = [
            {"op": "add", "path": "added", "content": "hello\n"},
            {"op": "update", "path": "update", "old_content": "before\n", "new_content": "after\n"},
            {"op": "delete", "path": "remove", "old_content": "gone\n"},
            {"op": "move", "from": "move", "to": "moved", "old_content": original["move"]},
        ]
        for result in [
            await self.patch.patch(changes),
            await self.patch.call({"changes": changes}),
        ]:
            self.assertTrue(result["data"]["dry_run"])
            self.assertTrue(result["data"]["completed"])
            self.assertEqual(result["data"]["applied"], [])
            self.assertIsNone(result["data"]["failure"])
            self.assertEqual(
                [p["operation"] for p in result["data"]["previews"]],
                ["add", "update", "delete", "move"],
            )
            self.assertEqual(
                {p.name: p.read_bytes() for p in self.root.iterdir()},
                {name: text.encode() for name, text in original.items()},
            )
        applied = (await self.patch.patch(changes, dry_run=False))["data"]
        self.assertFalse(applied["dry_run"])
        self.assertTrue(applied["completed"])
        self.assertEqual(
            [(e["operation"], e["path"]) for e in applied["applied"]],
            [
                ("add", "added"),
                ("update", "update"),
                ("delete", "remove"),
                ("add", "moved"),
                ("delete", "move"),
            ],
        )
        self.assertEqual(
            {p.name: p.read_bytes() for p in self.root.iterdir()},
            {"added": b"hello\n", "update": b"after\n", "moved": original["move"].encode()},
        )

    async def test_no_op_move_replacement_and_empty_file_existence(self) -> None:
        file = self.root / "source"
        file.write_bytes(b"original")
        before = file.stat()
        result = await self.patch.patch(
            [
                {
                    "op": "update",
                    "path": "source",
                    "old_content": "original",
                    "new_content": "original",
                }
            ],
            dry_run=False,
        )
        self.assertEqual(result["data"]["applied"], [])
        self.assertFalse(result["data"]["previews"][0]["changed"])
        self.assertEqual(file.stat().st_ino, before.st_ino)
        await self.patch.patch(
            [
                {
                    "op": "move",
                    "from": "source",
                    "to": "target",
                    "old_content": "original",
                    "new_content": "",
                },
                {"op": "add", "path": "empty", "content": ""},
            ],
            dry_run=False,
        )
        self.assertFalse(file.exists())
        self.assertEqual((self.root / "target").read_bytes(), b"")
        self.assertEqual((self.root / "empty").read_bytes(), b"")
        result = await self.patch.patch(
            [{"op": "delete", "path": "empty", "old_content": ""}], dry_run=False
        )
        self.assertTrue(result["data"]["previews"][0]["changed"])
        self.assertFalse((self.root / "empty").exists())

    async def test_whole_batch_validation_and_preview_revalidation(self) -> None:
        file = self.root / "file"
        file.write_text("old")
        changes = [
            {"op": "add", "path": "first", "content": "new"},
            {"op": "update", "path": "file", "old_content": "old", "new_content": "new"},
        ]
        await self.patch.patch(changes)
        file.write_text("external")
        with self.assertRaises(AccessRefusedError):
            await self.patch.patch(changes, dry_run=False)
        self.assertFalse((self.root / "first").exists())
        self.assertEqual(file.read_text(), "external")
        for change in [
            {"op": "add", "path": "file", "content": "new"},
            {"op": "delete", "path": "absent", "old_content": ""},
            {"op": "move", "from": "file", "to": "file", "old_content": "external"},
        ]:
            with self.assertRaises((AccessRefusedError, InvalidInputError)):
                await self.patch.patch([change], dry_run=False)
        duplicate = [{"op": "add", "path": name, "content": "x"} for name in ["new", "./new"]]
        with self.assertRaises(InvalidInputError):
            await self.patch.patch(duplicate, dry_run=False)
        self.assertEqual([p.name for p in self.root.iterdir()], ["file"])

    async def test_paths_symlinks_and_unsupported_text(self) -> None:
        (self.root / "good").write_text("good")
        for raw in [b"bad\0", b"\xff"]:
            (self.root / "bad").write_bytes(raw)
            with self.assertRaises(UnsupportedResourceError):
                await self.diff.diff("good", "bad")
        with self.assertRaises(UnsupportedResourceError):
            await self.diff.diff("good", ".")
        with self.assertRaises(ToolIOError):
            await self.diff.diff("good", "missing")
        with self.assertRaises(AccessRefusedError):
            await self.diff.diff("good", "..")
        (self.root / "link").symlink_to(self.root / "good")
        for change in [
            {"op": "add", "path": "../outside", "content": "x"},
            {"op": "update", "path": "link", "old_content": "good", "new_content": "x"},
        ]:
            with self.assertRaises(AccessRefusedError):
                await self.patch.patch([change], dry_run=False)
        with self.assertRaises(ToolIOError):
            await self.patch.patch(
                [{"op": "add", "path": "absent/file", "content": "x"}], dry_run=False
            )
        with self.assertRaises(UnsupportedResourceError):
            await self.patch.patch([{"op": "add", "path": "nul", "content": "\0"}], dry_run=False)
        self.assertEqual((self.root / "good").read_text(), "good")

    async def test_every_host_bound_and_strict_model_inputs(self) -> None:
        common = ["max_file_bytes", "max_lines", "max_comparison_cells", "max_output_bytes"]
        for cls, bounds in [
            (DiffTool, common),
            (PatchTool, common + ["max_changes", "max_input_bytes"]),
        ]:
            for bound in bounds:
                for value in [0, -1, True, "4", 2**64]:
                    with (
                        self.subTest(tool=cls.__name__, bound=bound, value=value),
                        self.assertRaises(InvalidInputError),
                    ):
                        await cls.create(self.workspace, **{bound: value})
            for value in [-1, True, "4", 2**64]:
                with self.assertRaises(InvalidInputError):
                    await cls.create(self.workspace, max_context_lines=value)
            await cls.create(self.workspace, max_context_lines=0)
        for arguments in [
            {"old_path": "a", "new_path": "b", "unknown": True},
            {"old_path": "a", "new_path": "b", "context_lines": True},
        ]:
            with self.assertRaises(InvalidInputError):
                await self.diff.call(arguments)
        change = {"op": "add", "path": "file", "content": "x"}
        for arguments in [
            {"changes": []},
            {"changes": [change], "dry_run": "false"},
            {"changes": [change], "extra": True},
            {"changes": [{**change, "extra": True}]},
            {"changes": [{"op": "update", "path": "file", "new_content": "x"}]},
            {"changes": [{"op": "rename", "path": "file"}]},
        ]:
            with self.assertRaises(InvalidInputError):
                await self.patch.call(arguments)
        self.assertEqual(list(self.root.iterdir()), [])

    async def test_resource_bounds_are_forwarded_and_context_is_capped(self) -> None:
        (self.root / "old").write_text("head\na\nb\ntail\n")
        (self.root / "new").write_text("head\nc\nd\ntail\n")
        changes = [{"op": "add", "path": "added", "content": "a\nb\n"}]
        for bound in ["max_file_bytes", "max_lines", "max_comparison_cells", "max_output_bytes"]:
            tool = await DiffTool.create(self.workspace, **{bound: 1})
            with self.subTest(bound=bound), self.assertRaises(LimitExceededError):
                await tool.diff("old", "new")
            tool = await PatchTool.create(self.workspace, **{bound: 1})
            # Comparison cells matter for two nonempty, different contents.
            batch = [
                {
                    "op": "update",
                    "path": "old",
                    "old_content": "head\na\nb\ntail\n",
                    "new_content": "head\nc\nd\ntail\n",
                }
            ]
            with self.subTest(patch_bound=bound), self.assertRaises(LimitExceededError):
                await tool.patch(batch, dry_run=False)
        tool = await PatchTool.create(self.workspace, max_input_bytes=1)
        with self.assertRaises(LimitExceededError):
            await tool.patch(changes, dry_run=False)
        tool = await PatchTool.create(self.workspace, max_changes=1)
        with self.assertRaises(InvalidInputError):
            await tool.patch(changes * 2, dry_run=False)
        tool = await DiffTool.create(self.workspace, max_context_lines=0)
        result = await tool.diff("old", "new", context_lines=10)
        self.assertEqual(result["data"]["context_lines"], 0)
        tool = await PatchTool.create(self.workspace, max_context_lines=0)
        result = await tool.patch(changes)
        self.assertEqual(result["data"]["previews"][0]["diff"]["context_lines"], 0)
        self.assertFalse((self.root / "added").exists())
        self.assertEqual((self.root / "old").read_text(), "head\na\nb\ntail\n")

    async def test_shared_workspace_serializes_patch_write_and_edit(self) -> None:
        (self.root / "file").write_text("old")
        rev = "sha256:" + hashlib.sha256(b"old").hexdigest()
        write = await WriteTool.create(self.workspace)
        edit = await EditTool.create(self.workspace)
        results = await asyncio.gather(
            self.patch.patch(
                [{"op": "update", "path": "file", "old_content": "old", "new_content": "patched"}],
                dry_run=False,
            ),
            write.write("file", "written", overwrite=True, expected_revision=rev),
            edit.edit("file", "old", "edited", expected_revision=rev),
            return_exceptions=True,
        )
        self.assertEqual(sum(isinstance(r, dict) for r in results), 1)
        self.assertEqual(sum(isinstance(r, AccessRefusedError) for r in results), 2)
        self.assertIn((self.root / "file").read_text(), {"patched", "written", "edited"})

    async def test_native_precancellation_and_python_cancellation(self) -> None:
        for native, root, args in [
            (_native._DiffTool, str(self.root), {"old_path": "old", "new_path": "new"}),
            (
                _native._PatchTool,
                self.workspace._native,
                {"changes": [{"op": "add", "path": "added", "content": "x"}], "dry_run": False},
            ),
        ]:
            cancel = _native._Cancellation()
            cancel.cancel()
            with self.assertRaises(asyncio.CancelledError):
                await native.create(root, "{}", cancel)
            tool = await native.create(root, "{}", _native._Cancellation())
            with self.assertRaises(asyncio.CancelledError):
                await tool.call(json.dumps(args), cancel)
        self.assertEqual(list(self.root.iterdir()), [])
        old, new = "a\n" * 4000, "b\n" * 4000
        (self.root / "old").write_text(old)
        (self.root / "new").write_text(new)
        diff = await DiffTool.create(self.workspace, max_comparison_cells=20_000_000)
        patch_tool = await PatchTool.create(self.workspace, max_comparison_cells=20_000_000)
        write = await WriteTool.create(self.workspace)
        for tool, arguments in [
            (diff, {"old_path": "old", "new_path": "new"}),
            (
                patch_tool,
                {
                    "changes": [
                        {"op": "update", "path": "old", "old_content": old, "new_content": new}
                    ],
                    "dry_run": False,
                },
            ),
        ]:
            cancel = _native._Cancellation()
            with patch("roomkit_tools.fs._native._Cancellation", return_value=cancel):
                task = asyncio.create_task(tool.call(arguments))
                await asyncio.sleep(0)
                self.assertFalse(task.done())
                task.cancel()
                with self.assertRaises(asyncio.CancelledError):
                    await task
            self.assertTrue(cancel.is_cancelled())
            await write.write("barrier", "done", overwrite=True)
            self.assertIn((self.root / "old").read_text(), [old, new])
            self.assertEqual(list(self.root.glob(".roomkit-write-*")), [])

    async def test_comparison_does_not_block_the_python_event_loop(self) -> None:
        (self.root / "old").write_text("a\n" * 4000)
        (self.root / "new").write_text("b\n" * 4000)
        tool = await DiffTool.create(self.workspace, max_comparison_cells=20_000_000)
        task = asyncio.create_task(tool.diff("old", "new"))
        ticks = 0
        async with asyncio.timeout(20):
            while not task.done():
                await asyncio.sleep(0.001)
                ticks += 1
        self.assertTrue((await task)["data"]["changed"])
        self.assertGreater(ticks, 1)


class DiffPatchLifecycleTests(unittest.TestCase):
    def test_root_retry_and_reuse_across_event_loops(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "later"
            workspace = Workspace(root)
            for cls in [DiffTool, PatchTool]:
                with self.assertRaises(ToolIOError):
                    asyncio.run(cls.create(workspace))
            root.mkdir()
            tool = asyncio.run(PatchTool.create(workspace))
            asyncio.run(
                tool.patch([{"op": "add", "path": "file", "content": "value"}], dry_run=False)
            )
            diff = asyncio.run(DiffTool.create(workspace))
            self.assertFalse(asyncio.run(diff.diff("file", "file"))["data"]["changed"])


if __name__ == "__main__":
    unittest.main()
