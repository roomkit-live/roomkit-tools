"""Interpreter exit with unfinished/cancelled native work must not crash or hang."""

from __future__ import annotations

import os
import subprocess
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path


class InterpreterShutdownTests(unittest.TestCase):
    def run_child(self, body: str, *, timeout: float = 30) -> subprocess.CompletedProcess[str]:
        with tempfile.TemporaryDirectory(prefix="roomkit shutdown-") as directory:
            root = Path(directory)
            (root / "input.txt").write_bytes(b"line\n" * (1024 * 1024))
            # An explicit list of arguments also covers paths containing spaces.
            result = subprocess.run(
                [
                    sys.executable,
                    "-X",
                    "faulthandler",
                    "-I",
                    "-c",
                    textwrap.dedent(body),
                    directory,
                ],
                capture_output=True,
                text=True,
                timeout=timeout,
                check=False,
            )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertNotIn("Fatal Python error", result.stderr)
        self.assertNotIn("Exception ignored", result.stderr)
        self.assertNotIn("Event loop is closed", result.stderr)
        return result

    def test_cancelled_reads_are_drained_before_interpreter_exit(self) -> None:
        # This sequence reproduced SIGSEGV with the untracked bridge, including
        # after its Python event loop had already returned successfully.
        for attempt in range(3):
            with self.subTest(attempt=attempt):
                self.run_child("""
                    import asyncio
                    import sys
                    from roomkit_tools import ReadTool, _native

                    async def main():
                        tool = await ReadTool.create(sys.argv[1])
                        pending = [
                            tool._native.read('{"path":"input.txt"}', _native._Cancellation())
                            for _ in range(128)
                        ]
                        for future in pending:
                            future.cancel()
                        return pending

                    sys.setswitchinterval(0.1)
                    pending = asyncio.run(main())
                """)

    def test_unawaited_reads_with_a_closed_loop_are_cancelled_on_exit(self) -> None:
        result = self.run_child("""
            import atexit
            import asyncio
            import sys

            def observe_shutdown():
                assert sentinel.is_cancelled()
                print('normal exit cancelled the parent scope')

            # Registered first, so this observer runs after the native hook.
            atexit.register(observe_shutdown)
            from roomkit_tools import ReadTool, _native
            sentinel = _native._Cancellation()

            async def main():
                tool = await ReadTool.create(sys.argv[1])
                return [tool._native.read('{"path":"input.txt"}', _native._Cancellation())
                        for _ in range(128)]

            sys.setswitchinterval(0.1)
            loop = asyncio.new_event_loop()
            pending = loop.run_until_complete(main())
            loop.close()
        """)
        self.assertIn("normal exit cancelled the parent scope", result.stdout)

    def test_completion_after_its_loop_closed_is_silent(self) -> None:
        if os.name != "posix":
            self.skipTest("the gate is a named pipe")
        # A host abandons calls, closes their loop and keeps running. Each call
        # waits on its own named pipe, so it completes only after the close.
        result = self.run_child("""
            import asyncio
            import json
            import os
            import sys
            import time
            from roomkit_tools import ShellTool, _native

            gates = [os.path.join(sys.argv[1], f'gate{index}') for index in range(4)]
            for gate in gates:
                os.mkfifo(gate)

            async def main():
                tool = await ShellTool.create(sys.argv[1])
                return [
                    tool._native.call(
                        json.dumps({'command': f'cat {os.path.basename(gate)}'}),
                        _native._Cancellation(),
                    )
                    for gate in gates
                ]

            loop = asyncio.new_event_loop()
            pending = loop.run_until_complete(main())
            loop.close()
            for gate in gates:
                with open(gate, 'w') as writer:
                    writer.write('done\\n')
            deadline = time.monotonic() + 20
            while _native._activity() and time.monotonic() < deadline:
                time.sleep(0.01)
            assert _native._activity() == 0, _native._activity()
            assert not any(future.done() for future in pending)
            print('completed after the loop closed')
        """)
        self.assertIn("completed after the loop closed", result.stdout)
        self.assertNotIn("Traceback", result.stderr)

    def test_child_forked_after_a_native_call_refuses_calls_at_once(self) -> None:
        if not hasattr(os, "fork"):
            self.skipTest("fork start method")
        result = self.run_child("""
            import asyncio
            import multiprocessing
            import sys
            import time
            from roomkit_tools import ReadTool, ToolError

            async def read_once():
                tool = await ReadTool.create(sys.argv[1])
                return (await tool.read('input.txt', limit=1))['text']

            def child(queue):
                started = time.monotonic()
                try:
                    asyncio.run(asyncio.wait_for(read_once(), 5))
                except BaseException as error:
                    elapsed = time.monotonic() - started
                    kind = type(error).__name__
                    queue.put((kind, isinstance(error, ToolError), str(error), elapsed))
                else:
                    queue.put(('read', False, '', 0.0))

            assert asyncio.run(read_once()).startswith('1: line\\n')
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
            assert asyncio.run(read_once()).startswith('1: line\\n')
        """)
        self.assertIn("ForkedProcessError: ", result.stdout)
        self.assertIn("spawn", result.stdout)

    def test_child_forked_during_a_call_exits_without_waiting_for_it(self) -> None:
        if not hasattr(os, "fork"):
            self.skipTest("os.fork")
        result = self.run_child("""
            import asyncio
            import json
            import os
            import signal
            import sys
            import time
            import warnings
            from roomkit_tools import ShellTool, ToolError, _native

            # Forking a process with native threads is the scenario under test.
            warnings.filterwarnings('ignore', category=DeprecationWarning)

            async def start():
                tool = await ShellTool.create(sys.argv[1])
                arguments = json.dumps({'command': 'sleep 5'})
                return tool._native.call(arguments, _native._Cancellation())

            loop = asyncio.new_event_loop()
            pending = loop.run_until_complete(start())
            assert _native._activity() > 0
            pid = os.fork()
            if pid == 0:
                try:
                    asyncio.run(asyncio.wait_for(ShellTool.create(sys.argv[1]), 5))
                except ToolError as error:
                    print('child refused:', type(error).__name__, flush=True)
                # A normal exit runs the inherited native exit hook.
                sys.exit(0)
            deadline = time.monotonic() + 15
            while (status := os.waitpid(pid, os.WNOHANG))[0] == 0:
                if time.monotonic() > deadline:
                    os.kill(pid, signal.SIGKILL)
                    os.waitpid(pid, 0)
                    raise SystemExit('the forked child hung at exit')
                time.sleep(0.01)
            assert os.waitstatus_to_exitcode(status[1]) == 0, status
            pending.cancel()
            loop.run_until_complete(asyncio.sleep(0))
            loop.close()
            print('forked child exited')
        """)
        self.assertIn("child refused: ForkedProcessError", result.stdout)
        self.assertIn("forked child exited", result.stdout)

    def test_close_in_a_forked_child_is_refused_before_any_effect(self) -> None:
        if not hasattr(os, "fork"):
            self.skipTest("os.fork")
        result = self.run_child("""
            import asyncio
            import os
            import sys
            import warnings
            from roomkit_tools import ForkedProcessError, OutputStore, ProcessSession

            # Forking a process with native threads is the scenario under test.
            warnings.filterwarnings('ignore', category=DeprecationWarning)

            async def open_resources():
                outputs = await OutputStore(sys.argv[1]).__aenter__()
                session = await ProcessSession(sys.argv[1], outputs=outputs).__aenter__()
                return session, outputs

            session, outputs = asyncio.run(open_resources())
            pid = os.fork()
            if pid == 0:
                for resource in (session, outputs):
                    name = type(resource).__name__
                    try:
                        asyncio.run(resource.close())
                    except ForkedProcessError:
                        print(f'{name} refused; closed={resource.closed}', flush=True)
                os._exit(0)
            assert os.waitstatus_to_exitcode(os.waitpid(pid, 0)[1]) == 0

            async def close():
                assert await session.close()
                assert await outputs.close()

            asyncio.run(close())
        """)
        self.assertIn("ProcessSession refused; closed=False", result.stdout)
        self.assertIn("OutputStore refused; closed=False", result.stdout)

    def test_importing_registers_no_fork_hook(self) -> None:
        if not hasattr(os, "register_at_fork"):
            self.skipTest("os.register_at_fork")
        # A native hook would run PyO3 code inside every child's fork().
        result = self.run_child("""
            import os

            hooks = []
            register = os.register_at_fork

            def recording(**callbacks):
                hooks.append(callbacks)
                register(**callbacks)

            os.register_at_fork = recording
            import roomkit_tools
            native = [
                callback
                for callbacks in hooks
                for callback in callbacks.values()
                if getattr(callback, '__module__', '').startswith('roomkit_tools')
            ]
            assert native == [], native
            print('no fork hook')
        """)
        self.assertIn("no fork hook", result.stdout)

    def test_children_forked_during_native_calls_never_hang(self) -> None:
        if not hasattr(os, "fork"):
            self.skipTest("os.fork")
        # Tokio workers briefly hold PyO3's lock of deferred releases while they
        # drop Python references. A child forked at that moment that runs PyO3
        # code waits for that lock forever. When fork() ran a native hook in the
        # child, about 1 fork in 1,200 hung under this load.
        result = self.run_child(
            """
            import asyncio
            import os
            import signal
            import sys
            import threading
            import time
            import warnings
            from pathlib import Path
            from roomkit_tools import ReadTool

            # Forking a process with native threads is the scenario under test.
            warnings.filterwarnings('ignore', category=DeprecationWarning)
            Path(sys.argv[1], 'small.txt').write_text('line\\n')
            stop = threading.Event()

            async def drive(ready):
                tool = await ReadTool.create(sys.argv[1])
                ready.set()
                while not stop.is_set():
                    await asyncio.gather(*(tool.read('small.txt') for _ in range(8)))

            def exited(pid):
                deadline = time.monotonic() + 2
                while os.waitpid(pid, os.WNOHANG)[0] == 0:
                    if time.monotonic() > deadline:
                        os.kill(pid, signal.SIGKILL)
                        os.waitpid(pid, 0)
                        return False
                    time.sleep(0.0002)
                return True

            drivers = []
            for _ in range(2):
                ready = threading.Event()
                driver = threading.Thread(target=asyncio.run, args=(drive(ready),))
                driver.start()
                drivers.append(driver)
                assert ready.wait(10)
            try:
                for attempt in range(5000):
                    pid = os.fork()
                    if pid == 0:
                        os._exit(0)
                    assert exited(pid), f'forked child {attempt} hung'
            finally:
                stop.set()
                for driver in drivers:
                    driver.join(10)
            print('every forked child exited')
            """,
            timeout=120,
        )
        self.assertIn("every forked child exited", result.stdout)

    def test_child_forked_before_any_native_call_can_use_tools(self) -> None:
        if not hasattr(os, "fork"):
            self.skipTest("fork start method")
        result = self.run_child("""
            import asyncio
            import multiprocessing
            import sys
            from roomkit_tools import ReadTool

            async def read_once():
                tool = await ReadTool.create(sys.argv[1])
                return (await tool.read('input.txt', limit=1))['text']

            def child(queue):
                queue.put(asyncio.run(asyncio.wait_for(read_once(), 5)))

            # Importing the package makes no native call.
            context = multiprocessing.get_context('fork')
            queue = context.Queue()
            process = context.Process(target=child, args=(queue,))
            process.start()
            print('child read:', queue.get(timeout=20).strip())
            process.join(20)
            assert process.exitcode == 0, process.exitcode
            print('parent read:', asyncio.run(read_once()).strip())
        """)
        self.assertIn("child read: 1: line", result.stdout)
        self.assertIn("parent read: 1: line", result.stdout)

    def test_shutdown_waits_for_activity_and_refuses_reopening(self) -> None:
        result = self.run_child("""
            import asyncio
            import sys
            from roomkit_tools import ReadTool, ToolError, _native

            async def main():
                tool = await ReadTool.create(sys.argv[1])
                # Keep the GIL here so native completions remain queued until
                # shutdown releases it; no timing sleep is used as a barrier.
                sys.setswitchinterval(10)
                pending = [tool._native.read('{"path":"input.txt"}', _native._Cancellation())
                           for _ in range(128)]
                for future in pending:
                    future.cancel()
                drained = _native._shutdown()
                assert drained > 0, drained
                assert _native._shutdown() == 0
                assert _native._Cancellation().is_cancelled()
                try:
                    await tool.read('input.txt')
                except ToolError as error:
                    assert 'shutting down' in str(error)
                else:
                    raise AssertionError('shutdown reopened the bridge')
                print('drained and permanently closed')

            asyncio.run(main())
        """)
        self.assertIn("drained and permanently closed", result.stdout)

    def test_import_only_shutdown_does_not_start_native_threads(self) -> None:
        if sys.platform != "linux":
            self.skipTest("observe Linux threads")
        result = self.run_child("""
            import os
            import roomkit_tools
            from roomkit_tools import _native
            assert len(os.listdir('/proc/self/task')) == 1
            assert _native._shutdown() == 0
            assert _native._shutdown() == 0
            assert len(os.listdir('/proc/self/task')) == 1
            print('no runtime needed')
        """)
        self.assertIn("no runtime needed", result.stdout)


if __name__ == "__main__":
    unittest.main()
