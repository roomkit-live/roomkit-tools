"""Read a temporary file through RoomKit and the native Rust tool.

From the repository root:
    uv venv
    uv pip install '.[roomkit]'
    .venv/bin/python examples/roomkit_read.py

The default model is scripted; the RoomKit turn and Rust file read are real.
For vLLM, also install 'roomkit[vllm]>=0.110,<0.112', set OPENAI_BASE_URL and
OPENAI_MODEL (optionally OPENAI_API_KEY), then add --live.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import tempfile
from pathlib import Path

from roomkit import (
    AIChannel,
    ChannelCategory,
    HookExecution,
    HookResult,
    HookTrigger,
    InboundMessage,
    RoomContext,
    RoomEvent,
    RoomKit,
    TextContent,
    ToolCallEvent,
    WebSocketChannel,
)
from roomkit.providers.ai import AIProvider, AIResponse, AIToolCall, MockAIProvider
from roomkit_tools import Read


def make_provider(live: bool) -> AIProvider:
    if live:
        from roomkit.providers.vllm import VLLMConfig, create_vllm_provider

        if not os.environ.get("OPENAI_BASE_URL") or not os.environ.get("OPENAI_MODEL"):
            raise SystemExit("Set OPENAI_BASE_URL (ending in /v1) and OPENAI_MODEL for --live.")
        return create_vllm_provider(
            VLLMConfig(
                base_url=os.environ["OPENAI_BASE_URL"],
                model=os.environ["OPENAI_MODEL"],
                api_key=os.environ.get("OPENAI_API_KEY") or "none",
                timeout=120.0,
                max_tokens=2048,
            )
        )
    return MockAIProvider(
        streaming=False,
        ai_responses=[
            AIResponse(
                content="",
                finish_reason="tool_calls",
                tool_calls=[AIToolCall(id="read-1", name="read", arguments={"path": "notes.txt"})],
            ),
            AIResponse(content="The Sample project is ready. (Scripted model response.)"),
        ],
    )


async def main(root: Path, live: bool) -> None:
    # The host chooses the directory and the tool; the model supplies only arguments.
    agent = AIChannel(
        "agent",
        provider=make_provider(live),
        tools=[Read(root=root, max_lines=20)],
        system_prompt="Use read to answer from notes.txt. Only report results actually returned.",
    )
    calls: list[ToolCallEvent] = []

    async with RoomKit() as kit:
        user = WebSocketChannel("user")

        # A local callback stands in for a browser; no HTTP/WebSocket server is started.
        async def show_reply(_connection: str, event: RoomEvent) -> None:
            if isinstance(event.content, TextContent):
                print(f"Assistant: {event.content.body}")

        user.register_connection("console", show_reply, room_id="demo")
        kit.register_channel(user)
        kit.register_channel(agent)
        await kit.create_room(room_id="demo")
        await kit.attach_channel("demo", "user")
        await kit.attach_channel("demo", "agent", category=ChannelCategory.INTELLIGENCE)

        @kit.hook(HookTrigger.ON_TOOL_CALL, execution=HookExecution.ASYNC)
        async def show_tool(event: ToolCallEvent, _context: RoomContext) -> HookResult:
            calls.append(event)
            print(f"Tool {event.name} (is_error={event.is_error}): {event.result}")
            return HookResult.allow()

        print("Live vLLM model" if live else "Scripted model; real RoomKit and Rust read")
        await kit.process_inbound(
            InboundMessage(
                channel_id="user",
                sender_id="reader",
                content=TextContent(body="Read notes.txt and tell me the project status."),
            ),
            room_id="demo",
        )

    # Check the actual tool result, not the scripted model's answer.
    if not live:
        assert len(calls) == 1 and calls[0].name == "read" and not calls[0].is_error
        assert json.loads(calls[0].result)["text"] == "1: Project: Sample\n2: Status: ready\n"


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--live", action="store_true", help="Use vLLM instead of a scripted model")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="roomkit-read-") as directory:
        root = Path(directory)
        (root / "notes.txt").write_text("Project: Sample\nStatus: ready\n", encoding="utf-8")
        asyncio.run(main(root, args.live))
