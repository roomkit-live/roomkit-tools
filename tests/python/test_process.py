"""Persistent process transport, isolation and owned session cleanup."""

from __future__ import annotations

import asyncio
import gc
import importlib.util
import json
import os
import sys
import tempfile
import unittest
import weakref
from pathlib import Path
from unittest.mock import patch

from process_helpers import COMMAND_WITH_CHILD, assert_stopped, read_pids
from roomkit_tools import (
    DEFAULT_INHERITED_ENV,
    AccessRefusedError,
    InvalidInputError,
    LimitExceededError,
    ProcessRead,
    ProcessSession,
    ProcessStart,
    ProcessStatus,
    ProcessStop,
    ProcessWrite,
    ToolIOError,
    Workspace,
    _native,
)


async def wait_status(session: ProcessSession, pid: str, **expected):
    async with asyncio.timeout(5):
        while True:
            status = (await session.status(pid))["data"][0]
            if all(status[key] == value for key, value in expected.items()):
                return status
            await asyncio.sleep(0.005)


class ProcessTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self) -> None:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.session = ProcessSession(Workspace(self.root))
        self.addAsyncCleanup(self.session.close)

    def test_schemas_selection_and_configuration_need_no_roomkit(self) -> None:
        self.assertIsNone(importlib.util.find_spec("roomkit"))
        for cls, name in [
            (ProcessStart, "process_start"),
            (ProcessStatus, "process_status"),
            (ProcessRead, "process_read"),
            (ProcessWrite, "process_write"),
            (ProcessStop, "process_stop"),
        ]:
            definition = ProcessSession.definition(name)
            self.assertEqual(definition, json.loads(_native._ProcessSession.definition_json(name)))
            self.assertEqual(cls(self.session).definition["parameters"], definition["input_schema"])
            self.assertFalse(definition["input_schema"]["additionalProperties"])
            self.assertNotIn("owner", definition["input_schema"]["properties"])
            definition["name"] = "changed"
            self.assertEqual(ProcessSession.definition(name)["name"], name)
        with self.assertRaises(InvalidInputError):
            ProcessSession.definition("shell")
        with self.assertRaises(TypeError):
            ProcessStart(self.root)
        with self.assertRaises(InvalidInputError):
            ProcessSession("")
        missing = ProcessSession(self.root / "later")
        self.assertFalse(missing.closed)
        self.assertFalse((self.root / "later").exists())

    async def test_cat_newline_read_with_open_stdin_then_explicit_eof(self) -> None:
        pid = (await self.session.start("cat"))["data"]["process_id"]
        written = (await self.session.write(pid, "Welcome\n"))["data"]
        self.assertEqual(written["bytes_written"], 8)
        self.assertTrue(written["complete"])
        self.assertFalse(written["stdin_closed"])
        await wait_status(self.session, pid, stdout_bytes=8)
        first = (await self.session.read(pid))["data"]
        again = (await self.session.read(pid))["data"]
        self.assertEqual(first["stdout"], again["stdout"])
        self.assertEqual(first["stdout"]["bytes"], list(b"Welcome\n"))
        self.assertEqual(first["stdout"]["next_offset"], 8)
        self.assertEqual(first["status"]["state"], "running")
        self.assertFalse(first["status"]["stdin_closed"])
        self.assertFalse(first["stdout"]["eof"])
        empty = (await self.session.read(pid, stdout_offset=8))["data"]
        self.assertEqual(empty["stdout"]["bytes"], [])
        self.assertEqual(empty["status"]["state"], "running")
        closed = (await self.session.write(pid, close_stdin=True))["data"]
        self.assertTrue(closed["stdin_closed"] and closed["complete"])
        final = await wait_status(self.session, pid, state="finished")
        self.assertEqual(final["exit_code"], 0)
        self.assertIsNone(final["signal"])
        self.assertEqual(final["end"], "exited")
        self.assertTrue(final["output_complete"])
        self.assertFalse((await self.session.write(pid, "late"))["data"]["complete"])

    async def test_byte_cursors_eviction_binary_and_independent_streams(self) -> None:
        async with ProcessSession(self.root, max_output_bytes=8, max_read_bytes=3) as session:
            pid = (await session.start("printf 0123456789abcdef; printf 'πxπ' >&2"))["data"][
                "process_id"
            ]
            await wait_status(session, pid, state="finished")
            first = (await session.read(pid, max_bytes=100))["data"]
            self.assertEqual(first["stdout"]["bytes"], list(b"89a"))
            self.assertEqual(first["stdout"]["omitted_bytes"], 8)
            self.assertEqual(first["stdout"]["offset"], 8)
            self.assertEqual(first["stdout"]["total_bytes"], 16)
            self.assertTrue(first["stdout"]["has_more"])
            self.assertEqual(first["stderr"]["bytes"], list("πx".encode()))
            second = (await session.read(pid, stdout_offset=11, stderr_offset=3))["data"]
            third = (await session.read(pid, stdout_offset=14, stderr_offset=5))["data"]
            raw = first["stdout"]["bytes"] + second["stdout"]["bytes"] + third["stdout"]["bytes"]
            self.assertEqual(bytes(raw), b"89abcdef")
            self.assertEqual(second["stderr"]["bytes"], list("π".encode()))
            self.assertEqual(third["stderr"]["bytes"], [])
            byte = (await session.read(pid, stderr_offset=0, max_bytes=1))["data"]["stderr"]
            self.assertEqual(byte["bytes"], [207])
            self.assertEqual(byte["text"], "\ufffd")
            with self.assertRaises(InvalidInputError):
                await session.read(pid, stdout_offset=17)

    async def test_separate_sessions_cannot_resolve_each_others_records(self) -> None:
        first = (await self.session.start("cat"))["data"]["process_id"]
        second = (await self.session.start("cat"))["data"]["process_id"]
        async with ProcessSession(self.root) as other:
            self.assertEqual((await other.status())["data"], [])
            with self.assertRaises(AccessRefusedError):
                await other.read(first)
            own = (await other.start("cat"))["data"]["process_id"]
            self.assertEqual(own, first)  # IDs are scope-local, not authorization tokens.
            for name, args in [
                ("process_status", {"process_id": second}),
                ("process_read", {"process_id": second}),
                ("process_write", {"process_id": second, "text": "x"}),
                ("process_stop", {"process_id": second}),
            ]:
                with self.assertRaises(AccessRefusedError):
                    await other.call(name, args)
        self.assertEqual(len((await self.session.status())["data"]), 2)

    async def test_host_secrets_stay_out_unless_listed_or_set_explicitly(self) -> None:
        command = 'printf "%s|%s" "${HOST_TEST_SECRET-unset}" "${GREETING-unset}"'
        with patch.dict(os.environ, {"HOST_TEST_SECRET": "s3cret"}):
            async with ProcessSession(
                self.root,
                inherit_env=(*DEFAULT_INHERITED_ENV, "HOST_TEST_SECRET"),
                env={"GREETING": "hello"},
            ) as configured:
                hidden = (await self.session.start(command))["data"]["process_id"]
                shown = (await configured.start(command))["data"]["process_id"]
                await wait_status(self.session, hidden, state="finished")
                await wait_status(configured, shown, state="finished")
                outputs = [
                    (await session.read(pid))["data"]["stdout"]["text"]
                    for session, pid in [(self.session, hidden), (configured, shown)]
                ]
        self.assertEqual(outputs, ["unset|unset", "s3cret|hello"])
        for options in [
            {"inherit_env": ["A=B"]},
            {"env": {"": "value"}},
            {"env": {"NAME": "a\0b"}},
        ]:
            with self.subTest(options=options):
                with self.assertRaises(InvalidInputError):
                    ProcessSession(self.root, **options)
        with self.assertRaises(TypeError):
            ProcessSession(self.root, inherit_env="PATH")

    async def test_finished_jobs_count_until_forget_and_ids_are_not_reused(self) -> None:
        async with ProcessSession(self.root, max_processes=2, max_processes_per_owner=1) as session:
            first = (await session.start("exit 7"))["data"]["process_id"]
            final = await wait_status(session, first, state="finished")
            self.assertEqual(final["exit_code"], 7)
            with self.assertRaises(LimitExceededError):
                await session.start("true")
            self.assertFalse((await session.stop(first))["data"]["forgotten"])
            self.assertTrue((await session.stop(first, forget=True))["data"]["forgotten"])
            with self.assertRaises(AccessRefusedError):
                await session.read(first)
            second = (await session.start("true"))["data"]["process_id"]
            self.assertNotEqual(second, first)

    async def test_all_host_limits_and_model_argument_types_are_validated_in_rust(self) -> None:
        fields = [
            "max_processes",
            "max_processes_per_owner",
            "max_command_bytes",
            "max_output_bytes",
            "max_read_bytes",
            "max_write_bytes",
            "write_timeout_ms",
            "default_timeout_ms",
            "max_timeout_ms",
        ]
        for field in fields:
            for value in [0, -1, True, "4", 2**64]:
                with self.subTest(field=field, value=value), self.assertRaises(InvalidInputError):
                    async with ProcessSession(self.root, **{field: value}):
                        pass
        for limits in [
            {"max_processes": 1},
            {"write_timeout_ms": 60_001},
            {"max_timeout_ms": 86_400_001},
            {"default_timeout_ms": 100, "max_timeout_ms": 50},
        ]:
            with self.assertRaises(InvalidInputError):
                async with ProcessSession(self.root, **limits):
                    pass
        for name, args in [
            ("process_start", {"command": "touch unexpected", "owner": "foreign"}),
            ("process_start", {"command": ""}),
            ("process_start", {"command": "\0"}),
            ("process_start", {"command": "true", "timeout_ms": 0}),
            ("process_start", {"command": "true", "timeout_ms": True}),
            ("process_status", {"owner": "foreign"}),
            ("process_read", {"process_id": "p1", "stdout_offset": -1}),
            ("process_read", {"process_id": "p1", "stderr_offset": True}),
            ("process_read", {"process_id": "p1", "max_bytes": 0}),
            ("process_write", {"process_id": "p1", "text": 1}),
            ("process_write", {"process_id": "p1", "close_stdin": "true"}),
            ("process_stop", {"process_id": "p1", "forget": 1}),
            ("unknown", {}),
        ]:
            with self.subTest(name=name, args=args), self.assertRaises(InvalidInputError):
                await self.session.call(name, args)
        self.assertFalse((self.root / "unexpected").exists())

    async def test_command_write_limits_and_cwd_boundaries(self) -> None:
        async with ProcessSession(self.root, max_command_bytes=3, max_write_bytes=2) as session:
            with self.assertRaises(LimitExceededError):
                await session.start("touch unexpected")
            pid = (await session.start("cat"))["data"]["process_id"]
            with self.assertRaises(LimitExceededError):
                await session.write(pid, "πx")
        (self.root / "sub").mkdir()
        before = Path.cwd()
        pid = (await self.session.start("pwd", cwd=Path("sub")))["data"]["process_id"]
        await wait_status(self.session, pid, state="finished")
        self.assertEqual(
            (await self.session.read(pid))["data"]["stdout"]["text"], str(self.root / "sub") + "\n"
        )
        self.assertEqual(Path.cwd(), before)
        with self.assertRaises(AccessRefusedError):
            await self.session.start("touch unexpected", cwd="..")
        with self.assertRaises(ToolIOError):
            await self.session.start("true", cwd="missing")

    async def test_lifetime_cap_and_signal_are_preserved(self) -> None:
        async with ProcessSession(self.root, default_timeout_ms=100, max_timeout_ms=200) as session:
            started = (await session.start("printf started; sleep 60", timeout_ms=10000))["data"]
            self.assertEqual(started["timeout_ms"], 200)
            final = await wait_status(session, started["process_id"], state="finished")
            self.assertEqual(final["end"], "timed_out")
            self.assertEqual(final["signal"], 9)
            self.assertIsNone(final["exit_code"])
            self.assertEqual(
                (await session.read(started["process_id"]))["data"]["stdout"]["text"], "started"
            )
            default = (await session.start("cat"))["data"]
            self.assertEqual(default["timeout_ms"], 100)
        pid = (await self.session.start("kill -TERM $$"))["data"]["process_id"]
        final = await wait_status(self.session, pid, state="finished")
        self.assertEqual(final["end"], "exited")
        self.assertEqual(final["signal"], 15)

    async def test_stdin_timeout_reports_partial_effects_without_stopping_job(self) -> None:
        async with ProcessSession(
            self.root, max_write_bytes=1024 * 1024, write_timeout_ms=30
        ) as session:
            pid = (await session.start("sleep 60"))["data"]["process_id"]
            result = (await session.write(pid, "x" * (1024 * 1024), close_stdin=True))["data"]
            self.assertFalse(result["complete"])
            self.assertGreater(result["bytes_written"], 0)
            self.assertLess(result["bytes_written"], 1024 * 1024)
            self.assertIn("timed out", result["error"])
            self.assertFalse(result["stdin_closed"])
            self.assertEqual((await session.status(pid))["data"][0]["state"], "running")

    async def test_native_and_python_write_cancellation_leave_job_owned(self) -> None:
        async with ProcessSession(
            self.root, max_write_bytes=1024 * 1024, write_timeout_ms=10000
        ) as session:
            pid = (await session.start("sleep 60"))["data"]["process_id"]
            arguments = {"process_id": pid, "text": "x" * (1024 * 1024)}
            cancel = _native._Cancellation()
            future = session._native.call("process_write", json.dumps(arguments), cancel)
            await asyncio.sleep(0.02)
            cancel.cancel()
            result = json.loads(await future)["data"]
            self.assertFalse(result["complete"])
            self.assertIn("cancelled", result["error"])
            cancel = _native._Cancellation()
            with patch("roomkit_tools.process._native._Cancellation", return_value=cancel):
                task = asyncio.create_task(session.write(pid, arguments["text"]))
                await asyncio.sleep(0)
                task.cancel()
                with self.assertRaises(asyncio.CancelledError):
                    await task
            self.assertTrue(cancel.is_cancelled())
            self.assertEqual((await session.status(pid))["data"][0]["state"], "running")

    async def test_precancelled_calls_and_lost_start_response_are_recoverable(self) -> None:
        cancel = _native._Cancellation()
        cancel.cancel()
        with self.assertRaises(asyncio.CancelledError):
            await self.session._native.initialize(cancel)
        with self.assertRaises(asyncio.CancelledError):
            await self.session._native.call(
                "process_start", '{"command":"touch unexpected"}', cancel
            )
        self.assertFalse((self.root / "unexpected").exists())
        task = asyncio.create_task(self.session.start("cat"))
        await asyncio.sleep(0)
        task.cancel()
        with self.assertRaises(asyncio.CancelledError):
            await task
        jobs = (await self.session.status())["data"]
        self.assertLessEqual(len(jobs), 1)
        for job in jobs:
            self.assertEqual(job["state"], "running")
            with self.assertRaises(asyncio.CancelledError):
                await self.session._native.call(
                    "process_read", json.dumps({"process_id": job["process_id"]}), cancel
                )
            self.assertEqual(
                (await self.session.status(job["process_id"]))["data"][0]["state"], "running"
            )

    @unittest.skipUnless(sys.platform == "linux", "observe Linux process cleanup")
    async def test_stop_close_and_dropped_close_reap_groups(self) -> None:
        for action in ["stop", "close", "cancel_close"]:
            session = ProcessSession(self.root)
            self.addAsyncCleanup(session.close)
            started = (await session.start(COMMAND_WITH_CHILD))["data"]
            ids = await read_pids(self.root / "pids")
            if action == "stop":
                result = (await session.stop(started["process_id"], forget=True))["data"]
                self.assertTrue(result["forgotten"])
                self.assertEqual(result["status"]["end"], "stopped")
            elif action == "close":
                self.assertTrue(await session.close())
            else:
                future = session._native.close()
                self.assertTrue(session.closed)
                future.cancel()
                with self.assertRaises(asyncio.CancelledError):
                    await future
            await assert_stopped(ids)
            self.assertTrue(await session.close())
            with self.assertRaises(AccessRefusedError):
                await session.start("true")
            (self.root / "pids").unlink()

    @unittest.skipUnless(sys.platform == "linux", "observe Linux process cleanup")
    async def test_context_exception_and_last_reference_drop_request_cleanup(self) -> None:
        with self.assertRaisesRegex(RuntimeError, "body failed"):
            async with ProcessSession(self.root) as session:
                await session.start(COMMAND_WITH_CHILD)
                ids = await read_pids(self.root / "pids")
                raise RuntimeError("body failed")
        await assert_stopped(ids)
        (self.root / "pids").unlink()
        session = ProcessSession(self.root)
        await session.start(COMMAND_WITH_CHILD)
        ids = await read_pids(self.root / "pids")
        reference = weakref.ref(session)
        del session
        gc.collect()
        self.assertIsNone(reference())
        await assert_stopped(ids)

    async def test_concurrent_first_calls_share_registry_and_closed_session_never_reopens(
        self,
    ) -> None:
        results = await asyncio.gather(*(self.session.start("cat") for _ in range(4)))
        self.assertEqual(len({r["data"]["process_id"] for r in results}), 4)
        self.assertEqual(len((await self.session.status())["data"]), 4)
        uninitialized = ProcessSession(self.root / "absent")
        self.assertTrue(await uninitialized.close())
        with self.assertRaises(AccessRefusedError):
            await uninitialized.status()
        racing = ProcessSession(self.root)
        self.addAsyncCleanup(racing.close)
        task = asyncio.create_task(racing.start("cat"))
        await asyncio.sleep(0)
        self.assertTrue(await racing.close())
        try:
            await task
        except AccessRefusedError:
            pass
        self.assertTrue(racing.closed)
        self.assertTrue(await racing.close())
        with self.assertRaises(AccessRefusedError):
            await racing.status()


class ProcessLifecycleTests(unittest.TestCase):
    def test_relative_root_retry_and_session_survives_python_event_loops(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            cwd = Path.cwd()
            try:
                os.chdir(directory)
                session = ProcessSession("later")
            finally:
                os.chdir(cwd)
            with self.assertRaises(ToolIOError):
                asyncio.run(session.status())
            Path(directory, "later").mkdir()
            pid = asyncio.run(session.start("cat"))["data"]["process_id"]
            asyncio.run(session.write(pid, "Hello\n"))
            asyncio.run(wait_status(session, pid, stdout_bytes=6))
            data = asyncio.run(session.read(pid))["data"]
            self.assertEqual(data["stdout"]["bytes"], list(b"Hello\n"))
            self.assertEqual(data["status"]["state"], "running")
            self.assertTrue(asyncio.run(session.close()))
            self.assertTrue(asyncio.run(session.close()))


if __name__ == "__main__":
    unittest.main()
