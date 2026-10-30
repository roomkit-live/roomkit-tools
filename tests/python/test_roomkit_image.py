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
from roomkit.providers.ai import AIImagePart, AITextPart
from roomkit.providers.vllm import VLLMConfig, create_vllm_provider
from roomkit.tools.compose import extract_tools
from roomkit_tools import ViewImage
from test_image import FIXTURE

EXAMPLE = Path(__file__).resolve().parents[2] / "examples/roomkit_image.py"


class RoomKitImageTests(unittest.IsolatedAsyncioTestCase):
    async def test_example_delivers_pixels_to_model_context(self):
        spec = importlib.util.spec_from_file_location("image_example", EXAMPLE)
        example = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(example)
        with tempfile.TemporaryDirectory() as root, redirect_stdout(io.StringIO()):
            calls, replies = await example.run(Path(root))
        self.assertEqual([e.name for e in calls], ["view_image"])
        self.assertIn("actual image part", replies[-1])

    async def test_selection_failures_and_concurrent_initialization(self):
        with tempfile.TemporaryDirectory() as root:
            await asyncio.to_thread(
                Path(root, "picture.png").write_bytes,
                base64.b64decode(FIXTURE["files"]["colors.png"]),
            )
            tool = ViewImage(root)
            definitions, handler = extract_tools([tool])
            self.assertEqual([d.name for d in definitions], ["view_image"])
            instances = await asyncio.gather(*(tool._get_tool() for _ in range(4)))
            self.assertTrue(all(t is instances[0] for t in instances))
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
                await ViewImage(root, max_image_bytes=1).handler(
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
                    AIChannel("assistant", provider=provider, tools=[ViewImage(root)])
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
