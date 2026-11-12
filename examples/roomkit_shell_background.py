"""An agent's long shell command runs on in the background, through RoomKit.

From the repository root:
    uv venv
    uv pip install '.[roomkit]'
    .venv/bin/python examples/roomkit_shell_background.py

The model is scripted; the RoomKit turns, the commands and the notice are real.
The agent runs a build with the shell tool. The build outlives the shell's
foreground timeout, so it moves to the background as a process instead of being
stopped, and the agent answers at once. When the build ends, notify_job_ends
tells the agent, which reads the rest of its output with process_read.
"""

from __future__ import annotations

import asyncio
import json
import tempfile

from roomkit import (
    AIChannel,
    ChannelCategory,
    InboundMessage,
    RoomKit,
    TextContent,
    WebSocketChannel,
)
from roomkit.providers.ai import AIResponse, AIToolCall, MockAIProvider
from roomkit_tools import ProcessRead, ProcessSession, Shell, ShellTool, notify_job_ends

BUILD = "echo compiling; sleep 0.6; echo built"


def call(call_id: str, name: str, arguments: dict) -> AIResponse:
    return AIResponse(
        content="", tool_calls=[AIToolCall(id=call_id, name=name, arguments=arguments)]
    )


def parts(messages: list) -> list:
    """Every part of the messages the model was sent."""
    return [
        part
        for message in messages
        if not isinstance(message.content, str)
        for part in message.content
    ]


def result(messages: list, tool: str) -> dict:
    """The last result of tool the model was sent, as the tool returned it."""
    found = [
        part.result
        for part in parts(messages)
        if getattr(part, "type", None) == "tool_result" and part.name == tool
    ]
    return json.loads(found[-1])


def notice(messages: list) -> str:
    """The line of the notice the agent was told."""
    texts = [message.content for message in messages if isinstance(message.content, str)]
    texts += [part.text for part in parts(messages) if hasattr(part, "text")]
    return next(
        line for text in texts for line in text.splitlines() if line.startswith("Background")
    )


async def main() -> None:
    provider = MockAIProvider(
        streaming=False,
        ai_responses=[
            call("c1", "shell", {"command": BUILD}),
            AIResponse(content="The build runs in the background; I will tell you when it ends."),
            call("c2", "process_read", {"process_id": "p1"}),
            AIResponse(content="The build finished."),
        ],
    )
    with tempfile.TemporaryDirectory() as root:
        async with ProcessSession(root) as session, RoomKit() as kit:
            # A foreground command still running after 300 ms moves to the session.
            shell = await ShellTool.create(
                root, processes=session, default_timeout_ms=300, background_on_timeout=True
            )
            tools = [Shell(shell), ProcessRead(session)]
            kit.register_channel(AIChannel("assistant", provider=provider, tools=tools))
            kit.register_channel(WebSocketChannel("chat"))
            await kit.create_room(room_id="room")
            await kit.attach_channel("room", "chat")
            await kit.attach_channel("room", "assistant", category=ChannelCategory.INTELLIGENCE)
            # Subscribe before the agent starts its jobs.
            telling = asyncio.create_task(notify_job_ends(kit, session.ends()))

            message = InboundMessage(
                channel_id="chat", sender_id="developer", content=TextContent(body="Build it.")
            )
            await kit.process_inbound(message, room_id="room")
            async with asyncio.timeout(5):
                while True:
                    if len(provider.calls) == 4:
                        break
                    await asyncio.sleep(0.05)

            moved = result(provider.calls[1].messages, "shell")
            told = notice(provider.calls[2].messages)
            rest = result(provider.calls[3].messages, "process_read")["data"]["stdout"]["text"]
            if moved["data"]["process_id"] != "p1" or moved["data"]["stdout"]["text"] != (
                "compiling\n"
            ):
                raise SystemExit(f"unexpected shell result: {moved}")
            if "p1" not in told or "exited with code 0" not in told or rest != "built\n":
                raise SystemExit(f"unexpected notice or output: {told!r}, {rest!r}")
            print(f"The shell returned: {moved['text'].splitlines()[0]}")
            print(f"The agent read: {told}")
            print(f"process_read gave the rest: {rest.strip()}")
        await asyncio.wait_for(telling, 5)


if __name__ == "__main__":
    asyncio.run(main())
