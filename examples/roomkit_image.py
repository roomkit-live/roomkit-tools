"""View actual image pixels through RoomKit; --live uses the configured vLLM.

The default scripted provider checks receipt of a multimodal image part. It
does not perform visual recognition. The generated fixture contains only shapes;
its contents are not described in the prompt or filename.
"""

import argparse
import asyncio
import json
import os
import struct
import tempfile
import zlib
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
from roomkit.providers.ai import (
    AIImagePart,
    AIResponse,
    AIToolCall,
    AIToolResultPart,
    MockAIProvider,
)
from roomkit_tools import (
    ViewImage,
    ViewImageTool,
)


def make_fixture(path):
    """Generate a deterministic PNG fixture using only the standard library."""

    def chunk(kind, data):
        return (
            struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
        )

    rows = bytearray()
    for y in range(300):
        rows.append(0)
        for x in range(900):
            color = (255, 255, 255)
            if 70 <= x < 230 and 70 <= y < 230:
                color = (220, 25, 25)
            elif (x - 450) ** 2 + (y - 150) ** 2 <= 80**2:
                color = (25, 60, 220)
            elif 70 <= y < 230 and abs(x - 750) <= (y - 70) // 2:
                color = (20, 155, 45)
            rows.extend(color)
    path.write_bytes(
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">2I5B", 900, 300, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(rows))
        + chunk(b"IEND", b"")
    )


class ScriptedVision(MockAIProvider):
    @property
    def supports_vision(self):
        return True

    async def generate(self, context):
        self.calls.append(context)
        images = [
            part
            for message in context.messages
            if isinstance(message.content, list)
            for result in message.content
            if isinstance(result, AIToolResultPart) and isinstance(result.result, list)
            for part in result.result
            if isinstance(part, AIImagePart)
        ]
        if not images:
            return AIResponse(
                content="",
                finish_reason="tool_calls",
                tool_calls=[
                    AIToolCall(id="view-1", name="view_image", arguments={"path": "sample.png"})
                ],
            )
        assert images[-1].url.startswith("data:image/png;base64,")
        return AIResponse(content="Scripted check: an actual image part reached the model context.")


def live_provider():
    from roomkit.providers.vllm import VLLMConfig, create_vllm_provider

    return create_vllm_provider(
        VLLMConfig(
            base_url=os.environ["OPENAI_BASE_URL"],
            model=os.environ["OPENAI_MODEL"],
            api_key=os.getenv("OPENAI_API_KEY") or "none",
            timeout=120,
            max_tokens=2048,
        )
    )


async def run(root, *, live=False):
    fixture = root / "sample.png"
    await asyncio.to_thread(make_fixture, fixture)
    original = fixture.read_bytes()
    calls, replies = [], []
    async with RoomKit() as kit:
        await kit.create_room(room_id="image-demo")

        @kit.hook(HookTrigger.ON_TOOL_CALL, execution=HookExecution.ASYNC)
        async def observe(event, _context):
            if event.name == "view_image":
                calls.append(event)
                if event.is_error:
                    print("view_image failed:", event.result)
                elif isinstance(event.result, list):
                    metadata = next(p.text for p in event.result if hasattr(p, "text"))
                    print("view_image metadata:", metadata)
                    print("Image pixels attached separately; base64 omitted from this log.")

        user = WebSocketChannel("user")

        async def display(_connection, event):
            if isinstance(event.content, TextContent):
                replies.append(event.content.body)
                print("Assistant:", event.content.body)

        user.register_connection("console", display, room_id="image-demo")
        kit.register_channel(user)
        kit.register_channel(
            AIChannel(
                "assistant",
                provider=live_provider() if live else ScriptedVision(),
                tools=[ViewImage(await ViewImageTool.create(root, max_edge=600))],
                system_prompt="Use view_image to inspect the actual pixels before answering.",
            )
        )
        await kit.attach_channel("image-demo", "user")
        await kit.attach_channel("image-demo", "assistant", category=ChannelCategory.INTELLIGENCE)
        async with asyncio.timeout(180):
            await kit.process_inbound(
                InboundMessage(
                    channel_id="user",
                    sender_id="reader",
                    content=TextContent(
                        body="Inspect sample.png with view_image. Describe the main shapes and "
                        "their colors, from left to right. Answer in English, "
                        "based only on the image."
                    ),
                ),
                room_id="image-demo",
            )
        successful = [e for e in calls if not e.is_error and isinstance(e.result, list)]
        assert successful, "no successful multimodal view_image call observed"
        parts = successful[-1].result
        assert any(
            isinstance(p, AIImagePart) and p.url.startswith("data:image/png;base64,") for p in parts
        )
        data = json.loads(next(p.text for p in parts if hasattr(p, "text")))["data"]
        assert (data["width"], data["height"], data["resized"]) == (600, 200, True)
        assert fixture.read_bytes() == original
        assert replies and replies[-1].strip(), "no final answer"
        print("Verified: actual image result, bounded preview and unchanged source.")
        if live:
            print(
                "Host reference for visual review: red square, blue circle, green triangle "
                "(left to right)."
            )
    return calls, replies


def cli(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--live", action="store_true")
    args = parser.parse_args(argv)
    print(
        "Live vision model"
        if args.live
        else "Scripted vision transport check; actual Rust image preparation"
    )
    with tempfile.TemporaryDirectory(prefix="roomkit-image-") as temporary:
        print("Temporary image workspace:", temporary)
        asyncio.run(run(Path(temporary), live=args.live))


if __name__ == "__main__":
    cli()
