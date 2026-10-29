"""Interpreter exit with unfinished/cancelled native work must not crash or hang."""

from __future__ import annotations

import subprocess
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path


class InterpreterShutdownTests(unittest.TestCase):
    def run_child(self, body: str) -> subprocess.CompletedProcess[str]:
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
                timeout=30,
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
