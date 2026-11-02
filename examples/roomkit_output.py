"""Recover a truncated log through RoomKit without rerunning the shell command.

The model is scripted by default; --live uses the configured vLLM endpoint.
All capture, archive reads, verification and cleanup use real native tools.
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
from roomkit.providers.ai import AIResponse, AIToolCall, MockAIProvider
from roomkit.providers.ai.base import AIContext, AIToolResultPart
from roomkit_tools import (
    OutputRead,
    OutputStore,
    Shell,
    ShellTool,
)

MIDDLE = "\nBuild error: missing dependency\n"
OFFSET = 65_536
COMMAND = "printf run >> runs; cat build.log"
TASK = (
    f"Run this exact shell command once: {COMMAND}. Its preview is intentionally short. "
    f"Then use output_read with the actual stdout archive_id, offset {OFFSET} and "
    f"max_bytes {len(MIDDLE.encode())} to retrieve the middle diagnostic. "
    "Report only the observed diagnostic and whether the archive is complete. "
    "Do not rerun shell to retrieve the log."
)


class ArchiveProvider(MockAIProvider):
    """Carry the observed archive ID into the next tool call."""

    def __init__(self) -> None:
        super().__init__(
            streaming=False,
            ai_responses=[
                AIResponse(
                    content="",
                    finish_reason="tool_calls",
                    tool_calls=[
                        AIToolCall(id="capture", name="shell", arguments={"command": COMMAND})
                    ],
                ),
                AIResponse(
                    content="",
                    finish_reason="tool_calls",
                    tool_calls=[
                        AIToolCall(
                            id="recover",
                            name="output_read",
                            arguments={"offset": OFFSET, "max_bytes": len(MIDDLE.encode())},
                        )
                    ],
                ),
                AIResponse(
                    content="The saved log reports a missing dependency; the archive is complete."
                ),
            ],
        )

    async def generate(self, context: AIContext) -> AIResponse:
        response = await super().generate(context)
        if response.tool_calls and response.tool_calls[0].name == "output_read":
            parts = [
                part
                for message in context.messages
                if isinstance(message.content, list)
                for part in message.content
                if isinstance(part, AIToolResultPart) and part.name == "shell" and not part.is_error
            ]
            response.tool_calls[0].arguments["archive_id"] = json.loads(parts[-1].result)["data"][
                "stdout"
            ]["archive"]["archive_id"]
        return response


def prepare(root: Path) -> None:
    (root / "build.log").write_bytes(("x" * OFFSET + MIDDLE + "y" * OFFSET).encode())


async def run(root: Path, provider) -> list[ToolCallEvent]:
    await asyncio.to_thread(prepare, root)
    events = []
    async with OutputStore() as outputs, RoomKit() as kit:
        agent = AIChannel(
            "agent",
            provider=provider,
            max_tool_rounds=6,
            tools=[
                Shell(await ShellTool.create(root, outputs=outputs, max_output_bytes=16)),
                OutputRead(outputs),
            ],
            system_prompt=f"You inspect a prepared diagnostic log in {root}. {TASK}",
        )
        kit.register_channel(agent)
        await kit.create_room(room_id="output-demo")
        await kit.attach_channel("output-demo", "agent", category=ChannelCategory.INTELLIGENCE)

        @kit.hook(HookTrigger.ON_TOOL_CALL, execution=HookExecution.ASYNC)
        async def observe(event: ToolCallEvent, _context: RoomContext) -> HookResult:
            if event.result is not None:
                events.append(event)
                print(f"Tool {event.name}: {event.result}")
            return HookResult.allow()

        user = WebSocketChannel("user")

        async def show(_connection: str, event: RoomEvent) -> None:
            if isinstance(event.content, TextContent):
                print(f"Assistant: {event.content.body}")

        user.register_connection("console", show, room_id="output-demo")
        kit.register_channel(user)
        await kit.attach_channel("output-demo", "user")
        await kit.process_inbound(
            InboundMessage(
                channel_id="user", sender_id="developer", content=TextContent(body=TASK)
            ),
            room_id="output-demo",
        )
        shells = [event for event in events if event.name == "shell" and not event.is_error]
        reads = [event for event in events if event.name == "output_read" and not event.is_error]
        if (
            len(shells) != 1
            or not reads
            or await asyncio.to_thread((root / "runs").read_bytes) != b"run"
        ):
            raise RuntimeError("Expected one real command followed by an archive read")
        stdout = json.loads(shells[0].result)["data"]["stdout"]
        if not stdout["truncated"] or not stdout["archive"]["complete"]:
            raise RuntimeError("Expected a truncated preview and complete disk copy")
        pages = [json.loads(event.result)["data"] for event in reads]
        if not any(
            page["archive"]["archive_id"] == stdout["archive"]["archive_id"]
            and page["offset"] == OFFSET
            and bytes(page["bytes"]) == MIDDLE.encode()
            for page in pages
        ):
            raise RuntimeError("The diagnostic was not recovered from the actual stdout archive")
        print(
            "Verified: one shell execution, truncated preview, "
            "exact middle bytes recovered from disk."
        )
    return events


def cli(argv: list[str] | None = None) -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--live", action="store_true", help="Use the configured vLLM endpoint")
    args = parser.parse_args(argv)
    provider = ArchiveProvider()
    if args.live:
        from roomkit.providers.vllm import VLLMConfig, create_vllm_provider

        if not os.getenv("OPENAI_BASE_URL") or not os.getenv("OPENAI_MODEL"):
            parser.error("Set OPENAI_BASE_URL and OPENAI_MODEL for live mode")
        provider = create_vllm_provider(
            VLLMConfig(
                base_url=os.environ["OPENAI_BASE_URL"],
                model=os.environ["OPENAI_MODEL"],
                api_key=os.getenv("OPENAI_API_KEY") or "none",
                timeout=120,
                max_tokens=2048,
            )
        )
    with tempfile.TemporaryDirectory(prefix="roomkit-output-demo-") as directory:
        print("Live vLLM" if args.live else "Scripted model; real RoomKit and Rust output archives")
        asyncio.run(run(Path(directory), provider))


if __name__ == "__main__":
    cli()
