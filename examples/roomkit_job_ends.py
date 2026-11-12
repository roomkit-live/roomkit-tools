"""An agent is told when its background process ends, through RoomKit.

From the repository root:
    uv venv
    uv pip install '.[roomkit]'
    .venv/bin/python examples/roomkit_job_ends.py

The model is scripted; the RoomKit turns, the process and the notice are real.
The agent starts a build in the background and answers at once; when the build
ends, notify_job_ends tells the agent, which reads the notice in a turn of its
own, without polling the process.
"""

from __future__ import annotations

import asyncio
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
from roomkit_tools import ProcessRead, ProcessSession, ProcessStart, notify_job_ends

BUILD = "sleep 0.5; echo built"


def texts(messages: list) -> list[str]:
    """The text of each message the model was sent."""
    found = []
    for message in messages:
        if isinstance(message.content, str):
            found.append(message.content)
        else:
            found += [part.text for part in message.content if hasattr(part, "text")]
    return found


async def main() -> None:
    provider = MockAIProvider(
        streaming=False,
        ai_responses=[
            AIResponse(
                content="",
                tool_calls=[
                    AIToolCall(id="c1", name="process_start", arguments={"command": BUILD})
                ],
            ),
            AIResponse(content="The build runs in the background; I will tell you when it ends."),
            AIResponse(content="The build finished: exit code 0."),
        ],
    )
    with tempfile.TemporaryDirectory() as root:
        async with ProcessSession(root) as session, RoomKit() as kit:
            tools = [ProcessStart(session), ProcessRead(session)]
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
            # The agent's answer, then the turn the build's end gives it.
            async with asyncio.timeout(5):
                while True:
                    if len(provider.calls) == 3:
                        break
                    await asyncio.sleep(0.05)

            notice = next(
                line
                for text in texts(provider.calls[2].messages)
                for line in text.splitlines()
                if line.startswith("Background process")
            )
            if "exited with code 0" not in notice:
                raise SystemExit(f"unexpected notice: {notice}")
            print(f"The agent read: {notice}")
            for event in await kit.store.list_events("room"):
                if event.source.channel_id == "assistant" and isinstance(
                    event.content, TextContent
                ):
                    print(f"The agent said: {event.content.body}")
        await asyncio.wait_for(telling, 5)


if __name__ == "__main__":
    asyncio.run(main())
