"""Multimodal result delivery, independent selection and actual vLLM wire format."""

import asyncio
import base64
import importlib.util
import io
import json
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path

import httpx
from roomkit import (
    AIChannel,
    ChannelCategory,
    InboundMessage,
    RoomKit,
    TextContent,
    WebSocketChannel,
)
from roomkit.core.exceptions import ToolFailedError, ToolRefusedError, UnservedToolCallError
from roomkit.providers.ai import (
    AIImagePart,
    AIResponse,
    AITextPart,
    AIToolCall,
    AIToolResultPart,
    MockAIProvider,
)
from roomkit.providers.vllm import VLLMConfig, create_vllm_provider
from roomkit.tools.compose import extract_tools
from roomkit_tools import (
    ViewImage,
    ViewImageTool,
)
from test_image import FIXTURE

EXAMPLE = Path(__file__).resolve().parents[2] / "examples/roomkit_image.py"
# The file and the exact results the RoomKit Rust adapter's tests read too.
SHARED = json.loads(
    (Path(__file__).resolve().parents[1] / "fixtures/roomkit_view_image.json").read_text()
)


def shared_workspace(root):
    target = Path(root, SHARED["path"])
    target.parent.mkdir()
    target.write_bytes(base64.b64decode(SHARED["png"]))


def model_calling_view_image(*, vision, path):
    return MockAIProvider(
        vision=vision,
        ai_responses=[
            AIResponse(
                content="",
                finish_reason="tool_calls",
                tool_calls=[AIToolCall(id="view-1", name="view_image", arguments={"path": path})],
            ),
            AIResponse(content="Done"),
        ],
    )


async def model_reads(root, *, vision, path):
    """What the model reads of its view_image call on path, through a real kit."""
    model = model_calling_view_image(vision=vision, path=path)
    async with asyncio.timeout(15), RoomKit() as kit:
        await kit.create_room(room_id="vision")
        kit.register_channel(
            AIChannel(
                "assistant", provider=model, tools=[ViewImage(await ViewImageTool.create(root))]
            )
        )
        user = WebSocketChannel("user")

        async def receive(_connection, _event):
            pass

        user.register_connection("console", receive, room_id="vision")
        kit.register_channel(user)
        await kit.attach_channel("vision", "assistant", category=ChannelCategory.INTELLIGENCE)
        await kit.attach_channel("vision", "user")
        await kit.process_inbound(
            InboundMessage(
                channel_id="user",
                sender_id="reader",
                content=TextContent(body="Inspect the image."),
            ),
            room_id="vision",
        )
    assert len(model.calls) == 2, "the model calls the tool, then answers"
    return next(
        part
        for message in model.calls[1].messages
        if isinstance(message.content, list)
        for part in message.content
        if isinstance(part, AIToolResultPart)
    )


class SharedFixtureTests(unittest.IsolatedAsyncioTestCase):
    """The results the RoomKit Rust adapter's tests compare with the same bytes."""

    async def asyncSetUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = directory.name
        await asyncio.to_thread(shared_workspace, self.root)

    def assert_shared_parts(self, parts):
        self.assertEqual(parts[0].text, SHARED["text_part"])
        self.assertEqual(parts[1].url, SHARED["image_part"]["url"])
        self.assertEqual(parts[1].mime_type, SHARED["image_part"]["mime_type"])
        self.assertEqual(len(parts), 2)

    async def test_answers_the_shared_text_part_and_image_part(self):
        tool = ViewImage(await ViewImageTool.create(self.root))
        self.assert_shared_parts(await tool.handler("view_image", {"path": SHARED["path"]}))

    async def test_refusals_and_failures_say_the_shared_words(self):
        tool = ViewImage(await ViewImageTool.create(self.root))
        for refusal in SHARED["refusals"]:
            with self.assertRaises(ToolRefusedError) as raised:
                await tool.handler("view_image", {"path": refusal["path"]})
            self.assertEqual(str(raised.exception), refusal["message"])
        failure = SHARED["failure"]
        limited = await ViewImageTool.create(self.root, max_image_bytes=failure["max_image_bytes"])
        with self.assertRaises(ToolFailedError) as raised:
            await ViewImage(limited).handler("view_image", {"path": SHARED["path"]})
        self.assertEqual(str(raised.exception), failure["message"])

    async def test_a_model_that_sees_receives_the_image_part_in_its_request(self):
        read = await model_reads(self.root, vision=True, path=SHARED["path"])
        self.assertFalse(read.is_error)
        self.assert_shared_parts(read.result)

    async def test_a_model_without_vision_reads_the_text_and_an_image_mark(self):
        read = await model_reads(self.root, vision=False, path=SHARED["path"])
        self.assertFalse(read.is_error)
        self.assertEqual(read.result, SHARED["text_part"] + "\n[image]")

    async def test_a_refused_path_reaches_the_model_in_the_shared_words(self):
        for refusal in SHARED["refusals"]:
            read = await model_reads(self.root, vision=True, path=refusal["path"])
            self.assertTrue(read.is_error, refusal)
            self.assertEqual(read.result, refusal["message"])


class RoomKitImageTests(unittest.IsolatedAsyncioTestCase):
    async def test_example_delivers_pixels_to_model_context(self):
        spec = importlib.util.spec_from_file_location("image_example", EXAMPLE)
        example = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(example)
        with tempfile.TemporaryDirectory() as root, redirect_stdout(io.StringIO()):
            calls, replies = await example.run(Path(root))
        self.assertEqual([e.name for e in calls], ["view_image"])
        self.assertIn("actual image part", replies[-1])

    async def test_selection_and_failures(self):
        with tempfile.TemporaryDirectory() as root:
            await asyncio.to_thread(
                Path(root, "picture.png").write_bytes,
                base64.b64decode(FIXTURE["files"]["colors.png"]),
            )
            tool = ViewImage(await ViewImageTool.create(root))
            definitions, handler = extract_tools([tool])
            self.assertEqual([d.name for d in definitions], ["view_image"])
            parts = await handler("view_image", {"path": "picture.png"})
            self.assertIsInstance(parts[0], AITextPart)
            self.assertIsInstance(parts[1], AIImagePart)
            self.assertNotIn("base64", parts[0].text)
            self.assertNotIn("image", json.loads(parts[0].text)["data"])
            with self.assertRaises(UnservedToolCallError):
                await handler("read", {"path": "picture.png"})
            with self.assertRaises(ToolRefusedError):
                await handler("view_image", {"path": ""})
            with self.assertRaises(ToolFailedError):
                await ViewImage(await ViewImageTool.create(root, max_image_bytes=1)).handler(
                    "view_image", {"path": "picture.png"}
                )

    async def test_vllm_receives_pixels_as_image_url_not_tool_text(self):
        requests = []

        async def serve(request):
            body = json.loads(request.content)
            requests.append(body)
            if len(requests) == 1:
                delta = {
                    "role": "assistant",
                    "tool_calls": [
                        {
                            "index": 0,
                            "id": "view-1",
                            "type": "function",
                            "function": {
                                "name": "view_image",
                                "arguments": '{"path":"picture.png"}',
                            },
                        }
                    ],
                }
                reason = "tool_calls"
            else:
                delta, reason = {"role": "assistant", "content": "Pixels received."}, "stop"

            def chunk(change, finish=None):
                return {
                    "id": "completion",
                    "object": "chat.completion.chunk",
                    "created": 0,
                    "model": "vision-fixture",
                    "choices": [{"index": 0, "delta": change, "finish_reason": finish}],
                }

            content = (
                "".join(
                    "data: " + json.dumps(value) + "\n\n"
                    for value in [chunk(delta), chunk({}, reason)]
                )
                + "data: [DONE]\n\n"
            )
            return httpx.Response(
                200, headers={"content-type": "text/event-stream"}, content=content
            )

        provider = create_vllm_provider(
            VLLMConfig(
                base_url="https://vision.invalid/v1",
                model="vision-fixture",
                api_key="fixture",
                max_retries=0,
            ),
            transport=httpx.MockTransport(serve),
        )
        async with asyncio.timeout(15), RoomKit() as kit:
            with tempfile.TemporaryDirectory() as root:
                await asyncio.to_thread(
                    Path(root, "picture.png").write_bytes,
                    base64.b64decode(FIXTURE["files"]["colors.png"]),
                )
                await kit.create_room(room_id="vision")
                kit.register_channel(
                    AIChannel(
                        "assistant",
                        provider=provider,
                        tools=[ViewImage(await ViewImageTool.create(root))],
                    )
                )
                user = WebSocketChannel("user")

                async def receive(_connection, _event):
                    pass

                user.register_connection("console", receive, room_id="vision")
                kit.register_channel(user)
                await kit.attach_channel(
                    "vision", "assistant", category=ChannelCategory.INTELLIGENCE
                )
                await kit.attach_channel("vision", "user")
                result = await kit.process_inbound(
                    InboundMessage(
                        channel_id="user",
                        sender_id="reader",
                        content=TextContent(body="Inspect picture.png"),
                    ),
                    room_id="vision",
                )
                self.assertIsNone(result.error)
        self.assertEqual(len(requests), 2)
        messages = requests[1]["messages"]
        images = [
            p
            for m in messages
            if isinstance(m["content"], list)
            for p in m["content"]
            if p["type"] == "image_url"
        ]
        self.assertEqual(len(images), 1)
        png = base64.b64decode(images[0]["image_url"]["url"].split(",", 1)[1], validate=True)
        self.assertTrue(png.startswith(b"\x89PNG\r\n\x1a\n"))
        tool_messages = [m for m in messages if m["role"] == "tool"]
        self.assertEqual(len(tool_messages), 1)
        self.assertEqual(tool_messages[0]["tool_call_id"], "view-1")
        self.assertNotIn("base64,", json.dumps(tool_messages))
