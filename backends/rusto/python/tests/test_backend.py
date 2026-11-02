"""Installed optional engine: contracts, typed failures and real-model parity."""

import asyncio
import hashlib
import json
import os
import subprocess
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path

from roomkit.core.exceptions import ToolRefusedError
from roomkit.tools.compose import extract_tools
from roomkit_tools import (
    AccessRefusedError,
    InvalidInputError,
    LimitExceededError,
    Ocr,
    OcrTool,
)
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
            definitions, handler = extract_tools(
                [Ocr(await OcrTool.create(FIXTURES, backend=backend))]
            )
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
        _, handler = extract_tools([Ocr(await OcrTool.create(FIXTURES, backend=backend))])
        self.assertEqual(json.loads(await handler("ocr", {"path": "page.png"})), result)
        self.assertEqual(await asyncio.to_thread((FIXTURES / "page.png").read_bytes), original)


# Child prelude: with an empty models directory, these calls fail before any
# model loads, after crossing the engine's own native bridge.
BRIDGE_PRELUDE = """
import asyncio
import sys
from roomkit_tools import InvalidInputError, OcrTool
from roomkit_tools_ocr_rusto import RustoOcr

backend = RustoOcr(sys.argv[1])

async def rejected_rotation():
    tool = await OcrTool.create(sys.argv[2], backend=backend)
    try:
        await tool.ocr('page.png', rotation_degrees=45)
    except InvalidInputError as error:
        return str(error)
    raise AssertionError('the rotation was accepted')
"""


class BridgeLifetimeTests(unittest.TestCase):
    """The engine's native bridge across fork() and closed event loops, without models."""

    def run_child(
        self, body: str, *, prelude: str = BRIDGE_PRELUDE
    ) -> subprocess.CompletedProcess[str]:
        with tempfile.TemporaryDirectory(prefix="roomkit ocr-") as models:
            result = subprocess.run(
                [
                    sys.executable,
                    "-X",
                    "faulthandler",
                    "-I",
                    "-c",
                    prelude + textwrap.dedent(body),
                    models,
                    str(FIXTURES),
                ],
                capture_output=True,
                text=True,
                timeout=30,
                check=False,
            )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertNotIn("Fatal Python error", result.stderr)
        self.assertNotIn("Exception ignored", result.stderr)
        self.assertNotIn("Event loop is closed", result.stderr)
        return result

    def test_completion_after_its_loop_closed_is_silent(self):
        # A host abandons calls, closes their loop and keeps running. These calls
        # fail too fast to close the loop under them, so each one starts with an
        # already closed loop as its running loop: it always completes after.
        result = self.run_child("""
            import json
            import time
            from roomkit_tools_ocr_rusto import _native

            bound = asyncio.run(backend.bind(sys.argv[2], {}))
            arguments = json.dumps({'path': 'page.png', 'rotation_degrees': 45})
            loop = asyncio.new_event_loop()
            loop.close()
            asyncio.events._set_running_loop(loop)
            try:
                pending = [
                    bound._native.call(arguments, _native.Cancellation()) for _ in range(4)
                ]
            finally:
                asyncio.events._set_running_loop(None)
            deadline = time.monotonic() + 20
            while _native._activity() and time.monotonic() < deadline:
                time.sleep(0.01)
            assert _native._activity() == 0, _native._activity()
            assert not any(future.done() for future in pending)
            print('completed after the loop closed')
        """)
        self.assertIn("completed after the loop closed", result.stdout)
        self.assertNotIn("Traceback", result.stderr)

    def test_child_forked_after_a_native_call_refuses_calls_at_once(self):
        if not hasattr(os, "fork"):
            self.skipTest("fork start method")
        result = self.run_child("""
            import multiprocessing
            import time
            from roomkit_tools import ToolError

            def child(queue):
                started = time.monotonic()
                try:
                    asyncio.run(asyncio.wait_for(rejected_rotation(), 5))
                except BaseException as error:
                    elapsed = time.monotonic() - started
                    kind = type(error).__name__
                    queue.put((kind, isinstance(error, ToolError), str(error), elapsed))
                else:
                    queue.put(('ocr', False, '', 0.0))

            expected = asyncio.run(rejected_rotation())
            context = multiprocessing.get_context('fork')
            queue = context.Queue()
            process = context.Process(target=child, args=(queue,))
            process.start()
            name, is_tool_error, message, elapsed = queue.get(timeout=20)
            process.join(20)
            print(f'{name}: {message}')
            assert is_tool_error, name
            assert elapsed < 1, elapsed
            assert process.exitcode == 0, process.exitcode
            # The parent keeps its runtime.
            assert asyncio.run(rejected_rotation()) == expected
        """)
        self.assertIn("ForkedProcessError: ", result.stdout)
        self.assertIn("spawn", result.stdout)

    def test_child_forked_during_a_call_exits_without_waiting_for_it(self):
        if not hasattr(os, "fork"):
            self.skipTest("os.fork")
        result = self.run_child("""
            import json
            import os
            import signal
            import time
            import warnings
            from roomkit_tools import ToolError
            from roomkit_tools_ocr_rusto import _native

            # Forking a process with native threads is the scenario under test.
            warnings.filterwarnings('ignore', category=DeprecationWarning)

            async def fork_during_call():
                bound = await backend.bind(sys.argv[2], {})
                arguments = json.dumps({'path': 'page.png', 'rotation_degrees': 45})
                # The call fails fast, but its completion needs the GIL: holding
                # the GIL until the fork keeps the call in flight at fork time.
                sys.setswitchinterval(60)
                pending = bound._native.call(arguments, _native.Cancellation())
                assert _native._activity() > 0
                pid = os.fork()
                if pid == 0:
                    try:
                        _native.Cancellation()
                    except ToolError as error:
                        print('child refused:', type(error).__name__, flush=True)
                    return pid, None
                sys.setswitchinterval(0.005)
                return pid, pending

            loop = asyncio.new_event_loop()
            pid, pending = loop.run_until_complete(fork_during_call())
            if pid == 0:
                # Leave the loop first: a normal exit then runs the inherited
                # native exit hook.
                sys.exit(0)
            deadline = time.monotonic() + 15
            while (status := os.waitpid(pid, os.WNOHANG))[0] == 0:
                if time.monotonic() > deadline:
                    os.kill(pid, signal.SIGKILL)
                    os.waitpid(pid, 0)
                    raise SystemExit('the forked child hung at exit')
                time.sleep(0.01)
            assert os.waitstatus_to_exitcode(status[1]) == 0, status
            # The child shared this loop's selector: never block on it again.
            pending.cancel()
            loop.run_until_complete(asyncio.sleep(0))
            loop.close()
            print('forked child exited')
        """)
        self.assertIn("child refused: ForkedProcessError", result.stdout)
        self.assertIn("forked child exited", result.stdout)

    def test_importing_registers_no_fork_hook(self):
        if not hasattr(os, "register_at_fork"):
            self.skipTest("os.register_at_fork")
        # A native hook would run PyO3 code inside every child's fork(). This
        # child records hooks before importing the engine: it has no prelude.
        result = self.run_child(
            """
            import os

            hooks = []
            register = os.register_at_fork

            def recording(**callbacks):
                hooks.append(callbacks)
                register(**callbacks)

            os.register_at_fork = recording
            import roomkit_tools_ocr_rusto
            native = [
                callback
                for callbacks in hooks
                for callback in callbacks.values()
                if getattr(callback, '__module__', '').startswith('roomkit_tools_ocr_rusto')
            ]
            assert native == [], native
            print('no fork hook')
            """,
            prelude="",
        )
        self.assertIn("no fork hook", result.stdout)

    def test_child_forked_before_any_native_call_can_use_the_engine(self):
        if not hasattr(os, "fork"):
            self.skipTest("fork start method")
        result = self.run_child("""
            import multiprocessing

            def child(queue):
                try:
                    queue.put(asyncio.run(asyncio.wait_for(rejected_rotation(), 5)))
                except BaseException as error:
                    queue.put(f'{type(error).__name__}: {error}')

            # The prelude only constructed the engine, which starts no thread.
            context = multiprocessing.get_context('fork')
            queue = context.Queue()
            process = context.Process(target=child, args=(queue,))
            process.start()
            print('child:', queue.get(timeout=20))
            process.join(20)
            assert process.exitcode == 0, process.exitcode
            print('parent:', asyncio.run(rejected_rotation()))
        """)
        self.assertIn("child: Invalid input: rotation_degrees", result.stdout)
        self.assertIn("parent: Invalid input: rotation_degrees", result.stdout)


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
