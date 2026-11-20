"""Shell selection, failures, cancellation and actual RoomKit agent execution."""

from __future__ import annotations

import asyncio
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from process_helpers import COMMAND_WITH_CHILD, assert_stopped, read_pids
from roomkit import AIChannel
from roomkit.core.exceptions import ToolFailedError, ToolRefusedError, UnservedToolCallError
from roomkit.models.channel import ChannelBinding
from roomkit.models.context import RoomContext
from roomkit.models.enums import ChannelCategory, ChannelType
from roomkit.models.event import EventSource, RoomEvent, TextContent
from roomkit.models.room import Room
from roomkit.providers.ai.base import AIResponse, AIToolCall, AIToolResultPart
from roomkit.providers.ai.mock import MockAIProvider
from roomkit.tools.compose import extract_tools
from roomkit_helpers import served
from roomkit_tools import (
    InvalidInputError,
    Shell,
    ShellTool,
    ToolIOError,
    Workspace,
)


class RoomKitShellTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self) -> None:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)

    async def test_individual_selection_schema_and_error_mapping(self) -> None:
        definitions, handler = extract_tools([Shell(await ShellTool.create(Workspace(self.root)))])
        self.assertEqual([d.name for d in definitions], ["shell"])
        self.assertEqual(definitions[0].parameters, ShellTool.definition()["input_schema"])
        with self.assertRaises(UnservedToolCallError):
            await handler("read", {"path": "notes"})
        for arguments in [
            {"command": ""},
            {"command": "pwd", "timeout_ms": 0},
            {"command": "pwd", "cwd": ".."},
        ]:
            with self.assertRaises(ToolRefusedError):
                await handler("shell", arguments)
        with self.assertRaises(ToolRefusedError):
            await handler("shell", {"command": "pwd", "cwd": "missing"})
        with self.assertRaises(ToolFailedError):
            await Shell(await ShellTool.create(self.root, max_command_bytes=1)).handler(
                "shell", {"command": "pwd"}
            )
        with self.assertRaises(InvalidInputError):
            await ShellTool.create(self.root, default_timeout_ms=0)
        bounded = Shell(
            await ShellTool.create(
                self.root, max_output_bytes=2, default_timeout_ms=50, max_timeout_ms=100
            )
        )
        with self.assertRaisesRegex(
            ToolRefusedError, "timeout_ms exceeds the host maximum of 100 ms"
        ):
            await bounded.handler("shell", {"command": "printf abcd", "timeout_ms": 200})
        _, data = await served(
            bounded.handler, "shell", {"command": "printf abcd", "timeout_ms": 100}
        )
        self.assertEqual(data["timeout_ms"], 100)
        self.assertEqual(data["stdout"]["omitted_bytes"], 2)

    async def test_environment_is_kept_as_validated_at_construction(self) -> None:
        command = 'printf "%s|%s" "${HOST_TEST_NAME-unset}" "${GREETING-unset}"'
        names, values = ["HOST_TEST_NAME"], {"GREETING": "hello"}
        listed = Shell(await ShellTool.create(self.root, inherit_env=names, env=values))
        generated = Shell(
            await ShellTool.create(self.root, inherit_env=(name for name in ["HOST_TEST_NAME"]))
        )
        # Later changes to the caller's objects do not reach the configured tool.
        names.append("BAD=NAME")
        values["BAD=NAME"] = "value"
        with patch.dict(os.environ, {"HOST_TEST_NAME": "host"}):
            results = [
                await served(shell.handler, "shell", {"command": command})
                for shell in (listed, generated)
            ]
        outputs = [data["stdout"]["text"] for _, data in results]
        self.assertEqual(outputs, ["host|hello", "host|unset"])
        # Like every tool's: the model reads the text alone.
        self.assertEqual(results[0][0], "exit_code=0\nstdout:\nhost|hello\n")

    async def test_agent_observes_output_nonzero_exit_and_timeout_as_results(self) -> None:
        (self.root / "notes").write_text("first\nsecond\n")
        arguments = [
            {"command": "cat notes"},
            {"command": "printf warning >&2; exit 7"},
            {"command": "printf started; sleep 60", "timeout_ms": 50},
        ]
        provider = MockAIProvider(
            ai_responses=[
                AIResponse(
                    content="",
                    finish_reason="tool_calls",
                    tool_calls=[AIToolCall(id=f"shell-{index}", name="shell", arguments=args)],
                )
                for index, args in enumerate(arguments)
            ]
            + [AIResponse(content="Read completed, command exited 7, last command timed out.")],
            streaming=False,
        )
        channel = AIChannel(
            "agent", provider=provider, tools=[Shell(await ShellTool.create(self.root))]
        )
        binding = ChannelBinding(
            room_id="test-room",
            channel_id="agent",
            channel_type=ChannelType.AI,
            category=ChannelCategory.INTELLIGENCE,
        )
        context = RoomContext(room=Room(id="test-room"), bindings=[binding])
        event = RoomEvent(
            room_id="test-room",
            source=EventSource(channel_id="user", channel_type=ChannelType.SMS),
            content=TextContent(body="Run the three commands and explain their results."),
        )
        try:
            response = await channel.on_event(event, binding, context)
            if response.response_stream is not None:
                async for _ in response.response_stream:
                    pass
        finally:
            await channel.close()
        self.assertEqual(len(provider.calls), 4)
        parts = [
            part
            for message in provider.calls[-1].messages
            if isinstance(message.content, list)
            for part in message.content
            if isinstance(part, AIToolResultPart)
        ]
        self.assertEqual(len(parts), 3)
        self.assertFalse(any(part.is_error for part in parts))
        # The model reads each command's text alone.
        self.assertEqual(
            [part.result for part in parts],
            [
                "exit_code=0\nstdout:\nfirst\nsecond\n",
                "exit_code=7\nstderr:\nwarning\n",
                "exit_code=null, signal=9, timed_out=true\nstdout:\nstarted\n",
            ],
        )

    @unittest.skipUnless(sys.platform == "linux", "observe Linux process cleanup")
    async def test_concurrent_first_calls_and_cancelled_handler_cleanup(self) -> None:
        shell = Shell(await ShellTool.create(self.root))
        long = asyncio.create_task(shell.handler("shell", {"command": COMMAND_WITH_CHILD}))
        short = asyncio.create_task(shell.handler("shell", {"command": "printf independent"}))
        try:
            ids = await read_pids(self.root / "pids")
            self.assertEqual(await short, "exit_code=0\nstdout:\nindependent\n")
            long.cancel()
            with self.assertRaises(asyncio.CancelledError):
                await long
            await assert_stopped(ids)
        finally:
            long.cancel()
            short.cancel()
        retry = await shell.handler("shell", {"command": "printf retry"})
        self.assertEqual(retry, "exit_code=0\nstdout:\nretry\n")


class RoomKitShellLifecycleTests(unittest.TestCase):
    def test_root_retry_and_cross_loop_use(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "later"
            with self.assertRaises(ToolIOError):
                asyncio.run(ShellTool.create(root))
            root.mkdir()
            shell = Shell(asyncio.run(ShellTool.create(root)))
            for _ in range(2):
                _, data = asyncio.run(served(shell.handler, "shell", {"command": "pwd"}))
                self.assertEqual(data["cwd"], ".")


if __name__ == "__main__":
    unittest.main()
