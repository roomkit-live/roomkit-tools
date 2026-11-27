"""Exercise the same contract with a Markdown and an injected backend."""

from pathlib import Path
from tempfile import TemporaryDirectory
from unittest import IsolatedAsyncioTestCase

from roomkit_tools import FileMemory, MemoryTool


class MemoryTests(IsolatedAsyncioTestCase):
    async def test_backends(self):
        with TemporaryDirectory() as root:
            notes = MemoryTool(FileMemory(root, 8192))
            saved = await notes.call(
                {"action": "save", "id": "ci", "title": "Runner", "text": "Blue host"}
            )
            self.assertIn("Blue host", (Path(root) / "ci.md").read_text())

            class Shared:
                async def execute(self, arguments):
                    if arguments != {"action": "search", "query": "ci"}:
                        raise AssertionError(arguments)
                    return saved["data"]

            result = await MemoryTool(Shared()).call({"action": "search", "query": "ci"})
            self.assertEqual(result, saved)
