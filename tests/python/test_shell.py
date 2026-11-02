"""Installed shell bindings preserve output, limits and owned process cleanup."""

from __future__ import annotations

import asyncio
import importlib.util
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from process_helpers import COMMAND_WITH_CHILD, assert_stopped, read_pids
from roomkit_tools import (
    DEFAULT_INHERITED_ENV,
    AccessRefusedError,
    InvalidInputError,
    LimitExceededError,
    Shell,
    ShellTool,
    ToolIOError,
    Workspace,
    _native,
)


class ShellTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self) -> None:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.tool = await ShellTool.create(self.root)

    def test_import_configuration_and_rust_schemas_without_roomkit(self) -> None:
        self.assertIsNone(importlib.util.find_spec("roomkit"))
        definition = ShellTool.definition()
        self.assertEqual(definition, json.loads(_native._ShellTool.definition_json()))
        self.assertEqual(definition["name"], "shell")
        self.assertEqual(
            set(definition["input_schema"]["properties"]), {"command", "cwd", "timeout_ms"}
        )
        self.assertFalse(definition["input_schema"]["additionalProperties"])
        self.assertIn("output_complete", definition["output_schema"]["properties"])
        facade = Shell(self.tool)
        self.assertEqual(facade.definition["parameters"], definition["input_schema"])
        definition["name"] = "changed"
        self.assertEqual(ShellTool.definition()["name"], "shell")

    async def test_shell_syntax_streams_and_nonzero_exit_are_results(self) -> None:
        (self.root / "notes").write_text("first\n")
        result = await self.tool.shell("cat notes | tr a-z A-Z; printf warning >&2; exit 7")
        data = result["data"]
        self.assertEqual(
            data["stdout"],
            {"text": "FIRST\n", "total_bytes": 6, "omitted_bytes": 0, "truncated": False},
        )
        self.assertEqual(data["stderr"]["text"], "warning")
        self.assertEqual(data["exit_code"], 7)
        self.assertIsNone(data["signal"])
        self.assertFalse(data["timed_out"])
        self.assertTrue(data["output_complete"])
        self.assertEqual(data["timeout_ms"], 10_000)
        self.assertIn("FIRST\n", result["text"])

    async def test_per_call_cwd_and_concurrent_calls_do_not_chdir_host(self) -> None:
        (self.root / "sub").mkdir()
        before = Path.cwd()
        a, b = await asyncio.gather(self.tool.shell("pwd", cwd=Path("sub")), self.tool.shell("pwd"))
        self.assertEqual(a["data"]["cwd"], "sub")
        self.assertEqual(a["data"]["stdout"]["text"], str(self.root / "sub") + "\n")
        self.assertEqual(b["data"]["cwd"], ".")
        self.assertEqual(Path.cwd(), before)
        absolute = await self.tool.shell("pwd", cwd=self.root / "sub")
        self.assertEqual(absolute["data"], a["data"])

    async def test_invalid_inputs_and_initial_paths_do_not_spawn(self) -> None:
        for args in [
            {"command": ""},
            {"command": " \n"},
            {"command": "bad\0command"},
            {"command": 7},
            {"command": "touch unexpected", "timeout_ms": 0},
            {"command": "touch unexpected", "timeout_ms": True},
            {"command": "touch unexpected", "timeout_ms": -1},
            {"command": "touch unexpected", "timeout_ms": 2**64},
            {"command": "touch unexpected", "cwd": False},
            {"command": "touch unexpected", "sandbox": True},
        ]:
            with self.subTest(args=args), self.assertRaises(InvalidInputError):
                await self.tool.call(args)
        with tempfile.TemporaryDirectory() as outside:
            (self.root / "escape").symlink_to(outside)
            for cwd in [outside, "escape", ".."]:
                with self.assertRaises(AccessRefusedError):
                    await self.tool.shell("touch unexpected", cwd=cwd)
            unexpected = Path(outside, "unexpected")
            self.assertFalse(await asyncio.to_thread(unexpected.exists))
        for cwd in ["missing", "file"]:
            (self.root / "file").write_text("regular file")
            with self.assertRaises(ToolIOError):
                await self.tool.shell("touch unexpected", cwd=cwd)
        self.assertFalse((self.root / "unexpected").exists())

    async def test_host_limits_are_strict_and_command_bound_is_enforced(self) -> None:
        for bound in [
            "max_command_bytes",
            "max_output_bytes",
            "default_timeout_ms",
            "max_timeout_ms",
        ]:
            for value in [0, -1, True, "10", 2**64]:
                with self.subTest(bound=bound, value=value), self.assertRaises(InvalidInputError):
                    await ShellTool.create(self.root, **{bound: value})
        for options in [
            {"default_timeout_ms": 20, "max_timeout_ms": 10},
            {"max_timeout_ms": 86_400_001},
        ]:
            with self.assertRaises(InvalidInputError):
                await ShellTool.create(self.root, **options)
        limited = await ShellTool.create(self.root, max_command_bytes=2)
        with self.assertRaises(LimitExceededError):
            await limited.shell("touch unexpected")
        self.assertFalse((self.root / "unexpected").exists())

    async def test_a_timeout_above_the_host_maximum_is_refused_with_it(self) -> None:
        limited = await ShellTool.create(self.root, default_timeout_ms=1000, max_timeout_ms=1000)
        with self.assertRaisesRegex(
            InvalidInputError, "timeout_ms exceeds the host maximum of 1000 ms"
        ):
            await limited.shell("touch unexpected", timeout_ms=1001)
        self.assertFalse((self.root / "unexpected").exists())
        result = await limited.shell("true", timeout_ms=1000)
        self.assertEqual(result["data"]["timeout_ms"], 1000)

    async def test_output_bounds_keep_head_tail_and_drain_both_pipes(self) -> None:
        tool = await ShellTool.create(self.root, max_output_bytes=100)
        result = await tool.shell(
            "printf HEAD; head -c 262144 /dev/zero; printf TAIL; "
            "printf ERR >&2; head -c 262144 /dev/zero >&2; printf END >&2"
        )
        data = result["data"]
        self.assertEqual(data["exit_code"], 0)
        self.assertTrue(data["output_complete"])
        for stream, total, start, end in [
            ("stdout", 262152, "HEAD", "TAIL"),
            ("stderr", 262150, "ERR", "END"),
        ]:
            captured = data[stream]
            self.assertEqual(captured["total_bytes"], total)
            self.assertEqual(captured["omitted_bytes"], total - 100)
            self.assertTrue(captured["truncated"])
            self.assertTrue(captured["text"].startswith(start))
            self.assertTrue(captured["text"].endswith(end))

    async def test_unicode_lossy_binary_stdin_eof_and_signal_exit(self) -> None:
        result = (await self.tool.shell("printf 'πxπ'; printf '\\377' >&2"))["data"]
        self.assertEqual(result["stdout"]["text"], "πxπ")
        self.assertEqual(result["stdout"]["total_bytes"], 5)
        self.assertEqual(result["stderr"]["text"], "\ufffd")
        self.assertEqual(result["stderr"]["total_bytes"], 1)
        eof = (await self.tool.shell("cat"))["data"]
        self.assertEqual(eof["exit_code"], 0)
        self.assertEqual(eof["stdout"]["total_bytes"], 0)
        signal = (await self.tool.shell("kill -TERM $$"))["data"]
        self.assertEqual(signal["signal"], 15)
        self.assertIsNone(signal["exit_code"])
        self.assertFalse(signal["timed_out"])

    async def test_timeout_default_maximum_partial_output_and_closed_pipes(self) -> None:
        tool = await ShellTool.create(self.root, default_timeout_ms=50, max_timeout_ms=100)
        for timeout, expected in [(None, 50), (100, 100)]:
            data = (await tool.shell("printf started; sleep 60", timeout_ms=timeout))["data"]
            self.assertTrue(data["timed_out"])
            self.assertEqual(data["timeout_ms"], expected)
            self.assertEqual(data["stdout"]["text"], "started")
            self.assertEqual(data["signal"], 9)
            self.assertIsNone(data["exit_code"])
            self.assertTrue(data["output_complete"])
        closed = (await tool.shell("exec 1>&- 2>&-; sleep 60"))["data"]
        self.assertTrue(closed["timed_out"])

    async def test_native_precancellation_never_spawns_and_retry_works(self) -> None:
        token = _native._Cancellation()
        token.cancel()
        with self.assertRaises(asyncio.CancelledError):
            await _native._ShellTool.create(str(self.root), "{}", token)
        with self.assertRaises(asyncio.CancelledError):
            await self.tool._native.call('{"command":"touch unexpected"}', token)
        self.assertFalse((self.root / "unexpected").exists())
        self.assertEqual((await self.tool.shell("printf retry"))["data"]["stdout"]["text"], "retry")

    @unittest.skipUnless(sys.platform == "linux", "observe Linux process cleanup")
    async def test_native_and_python_cancellation_stop_and_reap_processes(self) -> None:
        for python_cancellation in [False, True]:
            token = _native._Cancellation()
            with patch("roomkit_tools.process._native._Cancellation", return_value=token):
                if python_cancellation:
                    task = asyncio.create_task(self.tool.shell(COMMAND_WITH_CHILD))
                else:
                    task = asyncio.ensure_future(
                        self.tool._native.call(json.dumps({"command": COMMAND_WITH_CHILD}), token)
                    )
                try:
                    ids = await read_pids(self.root / "pids")
                    if python_cancellation:
                        task.cancel()
                    else:
                        token.cancel()
                    with self.assertRaises(asyncio.CancelledError):
                        await task
                finally:
                    token.cancel()
                    task.cancel()
                self.assertTrue(token.is_cancelled())
                await assert_stopped(ids)
            (self.root / "pids").unlink()
        self.assertEqual((await self.tool.shell("printf alive"))["data"]["exit_code"], 0)

    @unittest.skipUnless(sys.platform == "linux", "observe Linux process cleanup")
    async def test_timeout_and_normal_exit_stop_background_jobs(self) -> None:
        timed = await self.tool.shell("printf started; " + COMMAND_WITH_CHILD, timeout_ms=500)
        ids = await read_pids(self.root / "pids")
        self.assertTrue(timed["data"]["timed_out"])
        await assert_stopped(ids)
        (self.root / "pids").unlink()
        normal = await self.tool.shell("sleep 60 & printf '%s %s' $$ $! > pids; exit 0")
        self.assertEqual(normal["data"]["exit_code"], 0)
        await assert_stopped(await read_pids(self.root / "pids"))

    async def test_waiting_does_not_block_the_python_event_loop(self) -> None:
        task = asyncio.create_task(self.tool.shell("sleep 0.2; printf done"))
        ticks = 0
        async with asyncio.timeout(5):
            while not task.done():
                await asyncio.sleep(0.005)
                ticks += 1
        self.assertGreater(ticks, 2)
        self.assertEqual((await task)["data"]["stdout"]["text"], "done")

    async def test_workspace_is_only_initial_cwd_not_a_sandbox(self) -> None:
        (self.root / "inner").mkdir()
        (self.root / "outside").write_text("host access")
        tool = await ShellTool.create(Workspace(self.root / "inner"))
        data = (await tool.shell("cat ../outside"))["data"]
        self.assertEqual(data["stdout"]["text"], "host access")

    async def test_host_secrets_stay_out_unless_listed_or_set_explicitly(self) -> None:
        command = (
            'printf "%s|%s|%s" "${HOST_TEST_SECRET-unset}" "${GREETING-unset}" "${HOME:+home}"'
        )
        with patch.dict(os.environ, {"HOST_TEST_SECRET": "s3cret"}):
            default = (await self.tool.shell(command))["data"]["stdout"]["text"]
            listed = await ShellTool.create(
                self.root, inherit_env=(*DEFAULT_INHERITED_ENV, "HOST_TEST_SECRET")
            )
            explicit = await ShellTool.create(
                self.root, inherit_env=["HOST_TEST_SECRET"], env={"GREETING": "hello"}
            )
            overridden = await ShellTool.create(
                self.root, inherit_env=["HOST_TEST_SECRET"], env={"HOST_TEST_SECRET": "set"}
            )
            results = [
                (await tool.shell(command))["data"]["stdout"]["text"]
                for tool in (listed, explicit, overridden)
            ]
        self.assertEqual(default, "unset|unset|home")
        self.assertEqual(results, ["s3cret|unset|home", "s3cret|hello|", "set|unset|"])
        self.assertEqual(
            DEFAULT_INHERITED_ENV,
            ("PATH", "HOME", "USER", "LOGNAME", "SHELL", "LANG", "LC_ALL", "LC_CTYPE", "TZ")
            + ("TMPDIR",),
        )

    async def test_invalid_environment_is_refused_at_construction(self) -> None:
        for options in [
            {"inherit_env": [""]},
            {"inherit_env": ["A=B"]},
            {"inherit_env": ["A\0B"]},
            {"env": {"": "value"}},
            {"env": {"A=B": "value"}},
            {"env": {"NAME": "a\0b"}},
        ]:
            with self.subTest(options=options):
                with self.assertRaises(InvalidInputError):
                    await ShellTool.create(self.root, **options)
        for options in [{"inherit_env": "PATH"}, {"env": {"NAME": 1}}, {"env": [("A", "B")]}]:
            with self.subTest(options=options):
                with self.assertRaises(TypeError):
                    await ShellTool.create(self.root, **options)


class ShellLifecycleTests(unittest.TestCase):
    def test_root_retry_and_reuse_across_loops(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "later"
            with self.assertRaises(ToolIOError):
                asyncio.run(ShellTool.create(root))
            root.mkdir()
            tool = asyncio.run(ShellTool.create(root))
            self.assertEqual(asyncio.run(tool.shell("pwd"))["data"]["cwd"], ".")
            self.assertEqual(
                asyncio.run(tool.shell("printf reuse"))["data"]["stdout"]["text"], "reuse"
            )

    @unittest.skipUnless(sys.platform == "linux", "observe cleanup after Python loop closes")
    def test_loop_shutdown_requests_cleanup_in_the_native_runtime(self) -> None:
        with tempfile.TemporaryDirectory() as directory:

            async def start() -> list[int]:
                tool = await ShellTool.create(directory)
                task = asyncio.create_task(tool.shell(COMMAND_WITH_CHILD))
                ids = await read_pids(Path(directory, "pids"))
                self.assertFalse(task.done())
                # asyncio.run cancels the pending task on loop shutdown.
                return ids

            ids = asyncio.run(start())
            asyncio.run(assert_stopped(ids))


if __name__ == "__main__":
    unittest.main()
