"""Each native error an adapter maps, in the words the Rust adapters must give.

The expected words are in the fixture the RoomKit Rust adapter's
adapters/roomkit/tests/errors.rs reads too.
"""

import asyncio
import base64
import json
import tempfile
import unittest
from pathlib import Path

from roomkit import (
    AIChannel,
    ChannelCategory,
    HookExecution,
    HookTrigger,
    InboundMessage,
    RoomKit,
    TextContent,
    WebSocketChannel,
)
from roomkit.providers.ai import AIResponse, AIToolCall, AIToolResultPart, MockAIProvider
from roomkit_tools import Read, ReadTool, ViewImage, ViewImageTool

FIXTURE = json.loads(
    (Path(__file__).resolve().parents[1] / "fixtures/roomkit_tool_errors.json").read_text()
)


def write_workspace(root):
    for name, data in FIXTURE["files"].items():
        Path(root, name).write_bytes(base64.b64decode(data))


async def adapter(root, case):
    if case["tool"] == "view_image":
        return ViewImage(await ViewImageTool.create(root, **case["limits"]))
    return Read(await ReadTool.create(root, **case["limits"]))


def model_calling(case):
    call = AIToolCall(id="call-1", name=case["tool"], arguments=case["arguments"])
    return MockAIProvider(
        vision=True,
        ai_responses=[
            AIResponse(content="", finish_reason="tool_calls", tool_calls=[call]),
            AIResponse(content="Done"),
        ],
    )


async def read_through_a_kit(root, case):
    """What the model and the host's ON_TOOL_CALL hook read of a case's call."""
    model = model_calling(case)
    seen = []
    async with asyncio.timeout(15), RoomKit() as kit:
        await kit.create_room(room_id="errors")

        @kit.hook(HookTrigger.ON_TOOL_CALL, execution=HookExecution.ASYNC)
        async def observe(event, _context):
            seen.append(event)

        kit.register_channel(
            AIChannel("assistant", provider=model, tools=[await adapter(root, case)])
        )
        user = WebSocketChannel("user")

        async def receive(_connection, _event):
            pass

        user.register_connection("console", receive, room_id="errors")
        kit.register_channel(user)
        await kit.attach_channel("errors", "assistant", category=ChannelCategory.INTELLIGENCE)
        await kit.attach_channel("errors", "user")
        await kit.process_inbound(
            InboundMessage(
                channel_id="user", sender_id="reader", content=TextContent(body="Use the tool.")
            ),
            room_id="errors",
        )
    read = next(
        part
        for message in model.calls[1].messages
        if isinstance(message.content, list)
        for part in message.content
        if isinstance(part, AIToolResultPart)
    )
    return read, seen[-1]


class SharedErrorTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = directory.name
        await asyncio.to_thread(write_workspace, self.root)

    async def test_each_native_error_gives_the_handler_its_class_and_words(self):
        for case in FIXTURE["cases"]:
            with self.subTest(tool=case["tool"], variant=case["variant"]):
                tool = await adapter(self.root, case)
                with self.assertRaises(Exception) as raised:
                    await tool.handler(case["tool"], case["arguments"])
                self.assertEqual(type(raised.exception).__name__, case["raises"])
                self.assertEqual(str(raised.exception), case["message"])

    async def test_each_native_error_reaches_the_model_and_the_host_in_its_words(self):
        for case in FIXTURE["cases"]:
            with self.subTest(tool=case["tool"], variant=case["variant"]):
                read, event = await read_through_a_kit(self.root, case)
                self.assertTrue(read.is_error)
                self.assertEqual(read.result, case["model_reads"])
                self.assertTrue(event.is_error)
                self.assertEqual(event.result, case["model_reads"])
                self.assertEqual(event.error_detail, case["error_detail"])
                self.assertEqual(event.refused, case["refused"])
