"""One delegated read, with the report returned through the principal assistant.

Install roomkit-tools[roomkit], then run this file. The default providers are
scripted; delegation and the Rust read are real. For --live, also install
roomkit[vllm] and set OPENAI_BASE_URL, OPENAI_MODEL and optionally OPENAI_API_KEY.
See roomkit_agents.py for dynamic resolution, follow-ups and concurrent missions.
"""

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
    HookTrigger,
    InboundMessage,
    RoomKit,
    TextContent,
    WebSocketChannel,
)
from roomkit.providers.ai import AIResponse, AIToolCall, MockAIProvider
from roomkit_tools import (
    AgentScope,
    DelegateTask,
    DelegateTaskTool,
    Read,
    ReadTool,
)
from roomkit_tools.roomkit_agents import AgentConfig, RoomKitAgentBackend


def call(name, arguments):
    return AIResponse(
        content="",
        tool_calls=[AIToolCall(id=f"{name}-1", name=name, arguments=arguments)],
        finish_reason="tool_calls",
    )


def provider(live, responses):
    if not live:
        return MockAIProvider(ai_responses=responses)
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


async def main(root, *, live=False):
    scope = AgentScope("demo", "assistant")
    reads = []
    async with RoomKit() as kit:
        await kit.create_room(room_id="demo")

        @kit.hook(HookTrigger.ON_TOOL_CALL, execution=HookExecution.ASYNC)
        async def observe_read(event, _context):
            if event.name == "read" and not event.is_error:
                reads.append(json.loads(event.result))

        reader = await ReadTool.create(root)
        async with RoomKitAgentBackend(
            kit,
            scope=scope,
            profiles={
                "researcher": AgentConfig(
                    provider_factory=lambda: provider(
                        live,
                        [
                            call("read", {"path": "notes.txt"}),
                            AIResponse(content="notes.txt says: Project: Sample; Status: ready."),
                        ],
                    ),
                    system_prompt="Read notes.txt with read, then report its actual contents.",
                    tools_factory=lambda: [Read(reader)],
                )
            },
        ) as agents:
            assistant = AIChannel(
                "assistant",
                provider=provider(
                    live,
                    [
                        call("delegate_task", {"profile": "researcher", "task": "Read notes.txt"}),
                        AIResponse(content="The researcher is working."),
                        AIResponse(content="The researcher reports: the Sample project is ready."),
                    ],
                ),
                tools=[DelegateTask(DelegateTaskTool(agents))],
                system_prompt="Delegate the requested read to researcher. Acknowledge the launch "
                "and end your turn. When its report arrives, explain the result to the user.",
            )
            user = WebSocketChannel("user")

            # Local display callback: no WebSocket server is required.
            async def display(_connection, event):
                if isinstance(event.content, TextContent):
                    print("Assistant:", event.content.body)

            user.register_connection("console", display, room_id="demo")
            kit.register_channel(user)
            kit.register_channel(assistant)
            await kit.attach_channel("demo", "user")
            await kit.attach_channel("demo", "assistant", category=ChannelCategory.INTELLIGENCE)
            async with asyncio.timeout(180):
                await kit.process_inbound(
                    InboundMessage(
                        channel_id="user",
                        sender_id="reader",
                        content=TextContent(body="Ask researcher to read notes.txt."),
                    ),
                    room_id="demo",
                )
                tasks = (await agents.status(scope))["tasks"]
                assert len(tasks) == 1, "expected an actual delegation"
                result = await agents.wait(tasks[0]["task_id"])
            assert result["status"] == "completed" and result["delivery"] == "delivered"
            assert reads and reads[-1]["text"] == "1: Project: Sample\n2: Status: ready\n"
            print("Verified: delegated Rust read completed; report delivered to the principal.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--live", action="store_true")
    args = parser.parse_args()
    print("Live vLLM" if args.live else "Scripted providers; real RoomKit delegation and Rust read")
    with tempfile.TemporaryDirectory(prefix="roomkit-delegate-") as temporary:
        root = Path(temporary)
        (root / "notes.txt").write_text("Project: Sample\nStatus: ready\n", encoding="utf-8")
        asyncio.run(main(root, live=args.live))
