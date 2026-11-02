"""Real output archives preserve bytes, scope, quotas and explicit lifecycle."""

from __future__ import annotations

import asyncio
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

from roomkit_tools import (
    AccessRefusedError,
    InvalidInputError,
    OutputForget,
    OutputRead,
    OutputStore,
    ProcessSession,
    ShellTool,
    ToolIOError,
    _native,
)


async def wait_for(store, predicate, *, owner=None):
    async with asyncio.timeout(5):
        while True:
            entries = await store.list(owner=owner)
            if predicate(entries):
                return entries
            await asyncio.sleep(0.005)


class OutputTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self) -> None:
        directory = tempfile.TemporaryDirectory(prefix="output contracts ")
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.parent = self.root / "storage"
        self.parent.mkdir()
        self.store = OutputStore(self.parent)
        self.addAsyncCleanup(self.store.close)

    def test_configuration_selection_and_schemas_need_no_roomkit(self) -> None:
        self.assertIsNone(importlib.util.find_spec("roomkit"))
        self.assertEqual(list(self.parent.iterdir()), [])
        for facade, name in [(OutputRead, "output_read"), (OutputForget, "output_forget")]:
            definition = OutputStore.definition(name)
            self.assertEqual(definition, json.loads(_native._OutputStore.definition_json(name)))
            self.assertFalse(definition["input_schema"]["additionalProperties"])
            self.assertNotIn("owner", definition["input_schema"]["properties"])
            self.assertEqual(
                facade(self.store).definition["parameters"], definition["input_schema"]
            )
            with self.assertRaises(TypeError):
                facade(self.root)

    async def test_binary_copy_recovers_truncated_middle_without_a_second_command(self) -> None:
        original = bytes(range(256)) * 1024
        (self.root / "payload").write_bytes(original)
        shell = await ShellTool.create(self.root, outputs=self.store, max_output_bytes=8)
        result = (await shell.shell("printf run >> runs; cat payload; printf err >&2; exit 7"))[
            "data"
        ]
        self.assertEqual(result["exit_code"], 7)
        self.assertTrue(result["stdout"]["truncated"])
        archive = result["stdout"]["archive"]
        self.assertTrue(archive["complete"], archive)
        recovered = bytearray()
        while True:
            page = (
                await self.store.read(archive["archive_id"], offset=len(recovered), max_bytes=4096)
            )["data"]
            recovered.extend(page["bytes"])
            if not page["has_more"]:
                break
        self.assertEqual(recovered, original)
        self.assertEqual((self.root / "runs").read_bytes(), b"run")
        error = await self.store.read(result["stderr"]["archive"]["archive_id"])
        self.assertEqual(error["data"]["text"], "err")
        self.assertTrue(await self.store.close())
        self.assertEqual(list(self.parent.iterdir()), [])

    async def test_quota_prefix_paging_forget_and_unknown_ids(self) -> None:
        async with OutputStore(
            self.parent,
            max_stream_bytes=5,
            max_total_bytes=6,
            max_read_bytes=2,
            max_archives=2,
            max_archives_per_owner=2,
        ) as store:
            shell = await ShellTool.create(self.root, outputs=store)
            result = (await shell.shell("printf 0123456789"))["data"]
            archive = result["stdout"]["archive"]
            self.assertFalse(archive["complete"])
            self.assertEqual(archive["error"], "stream_limit")
            self.assertEqual(archive["stored_bytes"], 5)
            self.assertEqual(archive["observed_bytes"], 10)
            aid = archive["archive_id"]
            page = (await store.read(aid, max_bytes=100))["data"]
            self.assertEqual(page["bytes"], [48, 49])
            self.assertEqual(page["next_offset"], 2)
            with self.assertRaises(InvalidInputError):
                await store.read(aid, offset=6)
            second = (await shell.shell("printf still-runs"))["data"]
            self.assertEqual(second["stdout"]["text"], "still-runs")
            self.assertEqual(second["stdout"]["archive"]["error"], "archive_limit")
            self.assertTrue((await store.forget(aid))["data"]["forgotten"])
            with self.assertRaises(AccessRefusedError):
                await store.read(aid)
            third = (await shell.shell("printf ok"))["data"]
            self.assertTrue(third["stdout"]["archive"]["complete"])

    async def test_process_live_copy_and_logs_survive_process_session_close(self) -> None:
        async with ProcessSession(self.root, outputs=self.store, max_output_bytes=2) as session:
            started = (await session.start("cat"))["data"]
            aid = started["stdout_archive"]["archive_id"]
            pid = started["process_id"]
            await session.write(pid, "πabc\n")
            await wait_for(
                self.store,
                lambda entries: any(
                    e["archive_id"] == aid and e["stored_bytes"] == 6 for e in entries
                ),
            )
            page = (await self.store.read(aid, max_bytes=1))["data"]
            self.assertEqual(page["bytes"], [207])
            self.assertEqual(page["text"], "�")
            self.assertFalse(page["archive"]["eof"])
            with self.assertRaises(AccessRefusedError):
                await self.store.forget(aid)
            state = (await session.status(pid))["data"][0]
            self.assertFalse(state["stdin_closed"])
            self.assertEqual(state["stdout_retained_from"], 4)
        page = (await self.store.read(aid))["data"]
        self.assertTrue(page["archive"]["complete"])
        self.assertEqual(bytes(page["bytes"]), "πabc\n".encode())

    async def test_host_methods_reach_only_the_owner_they_name(self) -> None:
        async with (
            OutputStore(self.parent, max_archives_per_owner=2) as store,
            ProcessSession(self.root, outputs=store) as session,
        ):
            first = (await session.start("printf saved", owner="host-a"))["data"]
            aid = first["stdout_archive"]["archive_id"]
            await wait_for(
                store, lambda entries: all(e["finished"] for e in entries), owner="host-a"
            )
            self.assertEqual(await store.list(), [])
            listed = await store.list(owner="host-a")
            self.assertEqual([e["archive_id"] for e in listed], ["o1", "o2"])
            with self.assertRaises(AccessRefusedError):
                await store.read(aid)
            with self.assertRaises(AccessRefusedError):
                await store.forget(aid)
            page = (await store.read(aid, max_bytes=3, owner="host-a"))["data"]
            self.assertEqual(page["text"], "sav")
            call = await store.call("output_read", {"archive_id": aid}, owner="host-a")
            self.assertEqual(call["data"]["text"], "saved")
            # Both streams fill the owner's quota until the host forgets them.
            full = (await session.start("true", owner="host-a"))["data"]
            self.assertEqual(full["stdout_archive"]["error"], "archive_limit")
            for entry in listed:
                forgotten = await store.forget(entry["archive_id"], owner="host-a")
                self.assertTrue(forgotten["data"]["forgotten"])
            self.assertEqual(await store.list(owner="host-a"), [])
            again = (await session.start("true", owner="host-a"))["data"]
            self.assertIsNone(again["stdout_archive"]["error"])

    async def test_the_standalone_owner_is_apart_from_an_owner_named_default(self) -> None:
        async with (
            OutputStore(self.parent, max_archives_per_owner=2) as store,
            ProcessSession(self.root, outputs=store) as session,
        ):
            shell = await ShellTool.create(self.root, outputs=store)
            mine = (await shell.shell("printf mine"))["data"]["stdout"]["archive"]
            # The standalone owner's two archives fill its quota, not "default"'s.
            theirs = (await session.start("printf theirs", owner="default"))["data"]
            self.assertIsNone(theirs["stdout_archive"]["error"])
            self.assertEqual(len(await store.list()), 2)
            self.assertEqual(len(await store.list(owner="default")), 2)
            with self.assertRaises(AccessRefusedError):
                await store.read(mine["archive_id"], owner="default")

    async def test_separate_stores_isolate_ids_and_validate_inputs(self) -> None:
        shell = await ShellTool.create(self.root, outputs=self.store)
        result = (await shell.shell("printf private"))["data"]
        aid = result["stdout"]["archive"]["archive_id"]
        async with OutputStore(self.parent) as other:
            self.assertEqual(await other.list(), [])
            with self.assertRaises(AccessRefusedError):
                await other.read(aid)
            with self.assertRaises(AccessRefusedError):
                await other.forget(aid)
        for arguments in [
            {"archive_id": aid, "offset": True},
            {"archive_id": aid, "max_bytes": 0},
            {"archive_id": aid, "owner": "other"},
            {"path": "../../etc/passwd"},
        ]:
            with self.assertRaises(InvalidInputError):
                await self.store.call("output_read", arguments)
        with self.assertRaises(AccessRefusedError):
            await self.store.read("../../etc/passwd")

    async def test_storage_creation_failure_preserves_command_result(self) -> None:
        async with self.store:
            shell = await ShellTool.create(self.root, outputs=self.store)
            for directory in self.parent.iterdir():
                directory.rmdir()
            result = (await shell.shell("printf available; exit 7"))["data"]
            self.assertEqual(result["exit_code"], 7)
            self.assertEqual(result["stdout"]["text"], "available")
            archive = result["stdout"]["archive"]
            self.assertFalse(archive["complete"])
            self.assertEqual(archive["error"], "create_failed")
            self.assertEqual((await self.store.read(archive["archive_id"]))["data"]["bytes"], [])

    async def test_cancelled_shell_logs_can_be_recovered_by_host_list(self) -> None:
        shell = await ShellTool.create(self.root, outputs=self.store)
        task = asyncio.create_task(shell.shell("printf ready; sleep 30"))
        try:
            entries = await wait_for(
                self.store, lambda entries: any(e["stored_bytes"] == 5 for e in entries)
            )
            aid = next(e["archive_id"] for e in entries if e["stored_bytes"] == 5)
            task.cancel()
            with self.assertRaises(asyncio.CancelledError):
                await task
            await wait_for(self.store, lambda entries: all(e["finished"] for e in entries))
            self.assertEqual((await self.store.read(aid))["data"]["text"], "ready")
        finally:
            task.cancel()

    async def test_close_cannot_reopen_or_abandon_cleanup(self) -> None:
        shell = await ShellTool.create(self.root, outputs=self.store)
        await shell.shell("printf retained")
        future = self.store._native.close()
        future.cancel()
        self.assertTrue(self.store.closed)
        self.assertTrue(await self.store.close())
        self.assertEqual(list(self.parent.iterdir()), [])
        with self.assertRaises(AccessRefusedError):
            await self.store.list()
        result = (await shell.shell("printf continues"))["data"]
        self.assertEqual(result["stdout"]["text"], "continues")
        self.assertEqual(result["stdout"]["archive"]["error"], "store_closed")

    async def test_limits_and_precancellation_do_not_initialize_storage(self) -> None:
        for name in [
            "max_archives",
            "max_archives_per_owner",
            "max_stream_bytes",
            "max_total_bytes",
            "max_read_bytes",
            "write_timeout_ms",
        ]:
            store = OutputStore(self.parent, **{name: 0})
            try:
                with self.assertRaises(InvalidInputError):
                    await store.__aenter__()
            finally:
                await store.close()
        token = _native._Cancellation()
        token.cancel()
        with self.assertRaises(asyncio.CancelledError):
            await self.store._native.call("list", "{}", token)
        self.assertEqual(list(self.parent.iterdir()), [])


class OutputLifecycleTests(unittest.TestCase):
    def test_lazy_retry_and_reuse_across_event_loops(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            parent = Path(directory) / "later"
            store = OutputStore(parent)
            try:
                with self.assertRaises(ToolIOError):
                    asyncio.run(store.list())
                parent.mkdir()
                for _ in range(2):
                    self.assertEqual(asyncio.run(store.list()), [])
            finally:
                self.assertTrue(asyncio.run(store.close()))
