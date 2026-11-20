"""Archive tools stay independently selectable and execute through RoomKit."""

import asyncio
import importlib.util
import io
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path

from roomkit.core.exceptions import ToolRefusedError, UnservedToolCallError
from roomkit.tools.compose import extract_tools
from roomkit_helpers import in_call, served
from roomkit_tools import (
    OutputForget,
    OutputRead,
    OutputStore,
    Shell,
    ShellTool,
)

EXAMPLE_PATH = Path(__file__).resolve().parents[2] / "examples/roomkit_output.py"


class RoomKitOutputTests(unittest.IsolatedAsyncioTestCase):
    async def test_scripted_agent_recovers_middle_without_reexecution(self) -> None:
        spec = importlib.util.spec_from_file_location("roomkit_output_example", EXAMPLE_PATH)
        example = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(example)
        with tempfile.TemporaryDirectory() as directory, redirect_stdout(io.StringIO()) as output:
            provider = example.ArchiveProvider()
            events = await example.run(Path(directory), provider)
        self.assertEqual([event.name for event in events], ["shell", "output_read"])
        self.assertEqual(len(provider.calls), 3)
        self.assertIn("Verified:", output.getvalue())

    async def test_optional_forget_and_cross_store_refusal(self) -> None:
        with tempfile.TemporaryDirectory() as directory, in_call("room", "agent"):
            async with OutputStore(directory) as store, OutputStore(directory) as other:
                definitions, handler = extract_tools(
                    [
                        Shell(await ShellTool.create(directory, outputs=store, max_output_bytes=1)),
                        OutputRead(store),
                    ]
                )
                self.assertEqual([d.name for d in definitions], ["shell", "output_read"])
                text, data = await served(handler, "shell", {"command": "printf saved"})
                aid = data["stdout"]["archive"]["archive_id"]
                # The model reads the archive's ID in the text, to read it.
                self.assertIn(f"archive {aid}:", text)
                with self.assertRaises(UnservedToolCallError):
                    await handler("output_forget", {"archive_id": aid})
                with self.assertRaises(ToolRefusedError):
                    await OutputRead(other).handler("output_read", {"archive_id": aid})
                _, forgotten = await served(
                    OutputForget(store).handler, "output_forget", {"archive_id": aid}
                )
                self.assertTrue(forgotten["forgotten"])
                with self.assertRaises(ToolRefusedError):
                    await handler("output_read", {"archive_id": aid})

    async def test_rooms_sharing_a_store_only_reach_their_own_archives(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            async with OutputStore(directory) as store:
                tools = [
                    Shell(await ShellTool.create(directory, outputs=store, max_output_bytes=1)),
                    OutputRead(store),
                    OutputForget(store),
                ]
                _, handler = extract_tools(tools)
                with in_call("room-a", "agent"):
                    _, data = await served(handler, "shell", {"command": "printf private"})
                    archive = data["stdout"]["archive"]["archive_id"]
                for room, agent in [("room-b", "agent"), ("room-a", "other-agent")]:
                    with self.subTest(room=room, agent=agent), in_call(room, agent):
                        for name in ("output_read", "output_forget"):
                            with self.assertRaises(ToolRefusedError):
                                await handler(name, {"archive_id": archive})
                with in_call("room-a", "agent"):
                    _, page = await served(handler, "output_read", {"archive_id": archive})
                    self.assertEqual(page["text"], "private")
                for name, arguments in [
                    ("shell", {"command": "printf outside"}),
                    ("output_read", {"archive_id": archive}),
                ]:
                    with self.assertRaises(ToolRefusedError):
                        await handler(name, arguments)
                self.assertEqual(await store.list(), [])

    async def test_parallel_first_calls_share_one_store(self) -> None:
        with tempfile.TemporaryDirectory() as directory, in_call("room", "agent"):
            async with OutputStore(directory) as store:
                first, second = (
                    Shell(await ShellTool.create(directory, outputs=store, max_output_bytes=1)),
                    Shell(await ShellTool.create(directory, outputs=store, max_output_bytes=1)),
                )

                async def archived(shell, command):
                    # Each call has its own structured content, as in RoomKit.
                    with in_call("room", "agent"):
                        _, data = await served(shell.handler, "shell", {"command": command})
                    return data["stdout"]["archive"]["archive_id"]

                ids = await asyncio.gather(
                    archived(first, "printf first"), archived(second, "printf second")
                )
                self.assertNotEqual(*ids)
                reader = OutputRead(store)
                pages = [
                    (await served(reader.handler, "output_read", {"archive_id": aid}))[1]
                    for aid in ids
                ]
                self.assertEqual([page["text"] for page in pages], ["first", "second"])
