"""Selectable plan CRUD in RoomKit: deterministic check, or --live interactive chat.

The memory store survives turns in this process only. /plans shows actual state;
quit closes the conversation. Plan changes do not execute the described work.
"""

import argparse
import asyncio
import json
import os

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
from roomkit_tools import (
    MemoryPlanBackend,
    PlanCreate,
    PlanCreateTool,
    PlanDelete,
    PlanDeleteTool,
    PlanList,
    PlanListTool,
    PlanRead,
    PlanReadTool,
    PlanScope,
    PlanUpdate,
    PlanUpdateTool,
)

ROOM = "plans-demo"


class ScriptedPlanner(MockAIProvider):
    """Exercise real tools and use returned identifiers/revisions at every step."""

    async def generate(self, context):
        self.calls.append(context)
        parts = [
            part
            for message in context.messages
            if isinstance(message.content, list)
            for part in message.content
            if isinstance(part, AIToolResultPart)
        ]
        for part in parts:
            assert not part.is_error, part.result
        index = len(parts)
        if index == 0:
            name, arguments = (
                "plan_create",
                {
                    "title": "Review shipping",
                    "objective": "Check the shipping fee boundary",
                    "steps": [
                        {
                            "step_id": "inspect",
                            "description": "Read the source",
                            "status": "pending",
                        },
                        {
                            "step_id": "test",
                            "description": "Test the boundary",
                            "status": "pending",
                        },
                    ],
                },
            )
        elif index == 1:
            data = json.loads(parts[-1].result)["data"]
            name, arguments = "plan_read", {"plan_id": data["plan_id"]}
        elif index == 2:
            data = json.loads(parts[-1].result)["data"]
            steps = data["steps"]
            steps[0]["status"] = "in_progress"
            name, arguments = (
                "plan_update",
                {
                    "plan_id": data["plan_id"],
                    "expected_revision": data["revision"],
                    "steps": steps,
                },
            )
        elif index == 3:
            name, arguments = "plan_list", {}
        elif index == 4:
            data = json.loads(parts[-1].result)["data"]["plans"][0]
            assert data["revision"] == 2 and data["total_steps"] == 2
            name, arguments = (
                "plan_delete",
                {
                    "plan_id": data["plan_id"],
                    "expected_revision": data["revision"],
                },
            )
        elif index == 5:
            assert json.loads(parts[-1].result)["data"]["deleted"]
            name, arguments = "plan_list", {}
        else:
            assert json.loads(parts[-1].result)["data"]["plans"] == []
            return AIResponse(
                content="The plan was created, read, updated, listed and deleted. "
                "No shipping work was executed."
            )
        return AIResponse(
            content="",
            finish_reason="tool_calls",
            tool_calls=[AIToolCall(id=f"plan-{index}", name=name, arguments=arguments)],
        )


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


async def run(*, live=False):
    store = MemoryPlanBackend()
    observed = []
    async with RoomKit() as kit:
        await kit.create_room(room_id=ROOM)

        @kit.hook(HookTrigger.ON_TOOL_CALL, execution=HookExecution.ASYNC)
        async def observe(event, _context):
            observed.append(event)
            print(f"Tool {event.name} (error={event.is_error}): {event.result}")

        user = WebSocketChannel("user")

        async def display(_connection, event):
            if isinstance(event.content, TextContent):
                print("Assistant:", event.content.body)

        user.register_connection("console", display, room_id=ROOM)
        kit.register_channel(user)
        kit.register_channel(
            AIChannel(
                "assistant",
                provider=live_provider() if live else ScriptedPlanner(),
                tools=[
                    PlanCreate(PlanCreateTool(store)),
                    PlanRead(PlanReadTool(store)),
                    PlanList(PlanListTool(store)),
                    PlanUpdate(PlanUpdateTool(store)),
                    PlanDelete(PlanDeleteTool(store)),
                ],
                max_tool_rounds=12,
                system_prompt="Help the user create and track plans. "
                "Use the plan tools for actual actions. "
                "Read/list to recover current IDs and revisions; never invent them. "
                "Preserve stable step IDs. "
                "An update with steps replaces the whole list, including unchanged steps. "
                "These tools record declared progress only: you cannot execute the planned work. "
                "Only mark work completed when the user confirms it. "
                "Respond in the user's language.",
            )
        )
        await kit.attach_channel(ROOM, "user")
        await kit.attach_channel(ROOM, "assistant", category=ChannelCategory.INTELLIGENCE)
        if live:
            print("Plan chat. /plans: actual stored state. quit: close (memory is not persisted).")
            print("Try asking for a three-step plan to review a shipping fee function.")
        while True:
            message = (
                await asyncio.to_thread(input, "You: ") if live else "Exercise the plan CRUD tools."
            )
            if message.strip().lower() in ("quit", "exit"):
                break
            if not message.strip():
                continue
            if message.strip() == "/plans":
                scope = PlanScope(ROOM, "assistant")
                summaries = (await store.list(scope))["plans"]
                if not summaries:
                    print("Host: no plans in this conversation.")
                for summary in summaries:
                    print(
                        "Host:",
                        json.dumps(
                            await store.read(scope, plan_id=summary["plan_id"]), ensure_ascii=False
                        ),
                    )
                continue
            async with asyncio.timeout(180):
                result = await kit.process_inbound(
                    InboundMessage(
                        channel_id="user", sender_id="reader", content=TextContent(body=message)
                    ),
                    room_id=ROOM,
                )
            assert result.error is None, result.error
            if not live:
                assert [event.name for event in observed] == [
                    "plan_create",
                    "plan_read",
                    "plan_update",
                    "plan_list",
                    "plan_delete",
                    "plan_list",
                ]
                assert all(not event.is_error for event in observed)
                assert (await store.list(PlanScope(ROOM, "assistant")))["plans"] == []
                print(
                    "Verified: actual scoped CRUD results reached RoomKit; "
                    "the store is empty after deletion."
                )
                break


def cli():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--live", action="store_true")
    args = parser.parse_args()
    try:
        asyncio.run(run(live=args.live))
    except (EOFError, KeyboardInterrupt):
        pass


if __name__ == "__main__":
    cli()
