"""Static blueprints or dynamic resolution with the same five agent tools.

Default: scripted providers with real RoomKit and Rust reads. --live uses vLLM;
--chat is interactive. --dynamic selects the host resolver. Each delegation
creates a new agent instance; follow-ups retain its conversation and configuration.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import re
import tempfile
from collections import deque
from pathlib import Path
from uuid import uuid4

from roomkit import (
    AIChannel,
    ChannelCategory,
    CLIChannel,
    EventType,
    HookExecution,
    HookTrigger,
    InboundMessage,
    RoomKit,
    TextContent,
    WebSocketChannel,
)
from roomkit.providers.ai import AIResponse, AIToolCall, MockAIProvider
from roomkit.providers.ai.base import AIToolResultPart
from roomkit_tools import (
    AgentFollowup,
    AgentFollowupTool,
    AgentScope,
    AgentSend,
    AgentSendTool,
    CancelTask,
    CancelTaskTool,
    CodeOutline,
    DelegateTask,
    DelegateTaskTool,
    Grep,
    Read,
    TaskStatus,
    TaskStatusTool,
    ToolError,
)
from roomkit_tools.roomkit_agents import AgentConfig, RoomKitAgentBackend

ROOM = "agents-demo"
SOURCE = '''def shipping_fee(subtotal: int) -> int:
    """Shipping is free for orders of 50 units or more."""
    if subtotal > 50:
        return 0
    return 5
'''
TASK = (
    "Read shipping.py using read. Identify the boundary bug and suggest the smallest correction. "
    "Cite the observed source line. Do not modify files or claim tests were run."
)

FOLLOWUP_TASK = (
    "Continue your earlier analysis. Make a fresh read call on shipping.py with offset=3, "
    "limit=1 now; previous tool results do not satisfy this new mission. "
    "Cite the line returned by this call, and propose three boundary test inputs. "
    "Do not modify files or claim tests were run."
)


class ScriptedWorker(MockAIProvider):
    async def generate(self, context):
        reads = [
            json.loads(part.result)
            for message in context.messages
            if isinstance(message.content, list)
            for part in message.content
            if isinstance(part, AIToolResultPart) and part.name == "read" and not part.is_error
        ]
        if context.messages[-1].role != "tool" or not reads:
            return tool_call("read", {"path": "shipping.py"})
        line = next(line for line in reads[-1]["data"]["lines"] if "subtotal >" in line["text"])
        markers = re.findall(r"PARALLEL_[0-9a-f]{12}", str(context.messages))
        acknowledgement = f" Verification marker: {markers[-1]}." if markers else ""
        return AIResponse(
            content=f"shipping.py:{line['number']}: {line['text'].strip()}. "
            "Exactly 50 is excluded. Suggested correction: >= 50. No files were modified."
            + acknowledgement
        )


def tool_call(name, arguments):
    return AIResponse(
        content="",
        finish_reason="tool_calls",
        tool_calls=[AIToolCall(id=uuid4().hex, name=name, arguments=arguments)],
    )


class ScriptedSpeaker(MockAIProvider):
    async def generate(self, context):
        last = context.messages[-1]
        if last.role == "tool":
            return AIResponse(content="Observed: " + str(last.content[0].result))
        text = str(last.content)
        if "<worker_output>" in text:
            report = text.split("<worker_output>", 1)[1].split("</worker_output>", 1)[0].strip()
            return AIResponse(content="The worker found: " + report)
        if text.startswith("Continue "):
            return tool_call("agent_followup", {"agent_id": text.split()[1], "task": FOLLOWUP_TASK})
        if text.startswith("Start "):
            return tool_call("delegate_task", {"profile": text.split()[1], "task": TASK})
        return AIResponse(
            content=(
                "A profile is a blueprint; delegate_task creates an instance, "
                "agent_followup reuses it."
            )
        )


def live_provider():
    from roomkit.providers.vllm import VLLMConfig, create_vllm_provider

    if not os.getenv("OPENAI_BASE_URL") or not os.getenv("OPENAI_MODEL"):
        raise RuntimeError("set OPENAI_BASE_URL and OPENAI_MODEL for vLLM")
    return create_vllm_provider(
        VLLMConfig(
            base_url=os.environ["OPENAI_BASE_URL"],
            model=os.environ["OPENAI_MODEL"],
            api_key=os.getenv("OPENAI_API_KEY") or "none",
            timeout=120,
            max_tokens=2048,
        )
    )


class TurnDiagnostics:
    """Bounded observations from public RoomKit hooks, without prompt/error bodies."""

    def __init__(self, kit):
        self.records = deque(maxlen=32)

        @kit.hook(HookTrigger.ON_AI_RESPONSE, execution=HookExecution.ASYNC)
        async def observed_response(event, _context):
            record = {
                "kind": "response",
                "channel_id": event.channel_id,
                "room_id": event.room_id,
                "end_reason": event.loop_end_reason,
                "text_chars": len(event.response_content.strip()),
                "tool_calls": event.tool_calls_count,
                "rounds": event.round_count,
                "latency_ms": event.latency_ms,
            }
            self.records.append(record)
            if not record["text_chars"] or record["end_reason"] != "completed":
                print("Host turn diagnostic:", json.dumps(record))

        @kit.hook(HookTrigger.ON_ERROR, execution=HookExecution.ASYNC)
        async def observed_error(event, _context):
            # Provider errors may embed URLs, credentials or request bodies.
            # Surface their classification only; never print raw error text.
            record = {
                "kind": "error",
                "channel_id": event.source.channel_id,
                "room_id": event.room_id,
                "error_type": str(event.metadata.get("error_type", "unknown"))[:80],
                "error_category": str(event.metadata.get("error_category", "unknown"))[:80],
            }
            self.records.append(record)
            print("Host turn diagnostic:", json.dumps(record))

    def show(self):
        if not self.records:
            print("Host: no turn completion or error observed yet.")
        for record in self.records:
            print("Host turn:", json.dumps(record))


class PausedRead:
    """An instance-owned demo gate; the actual Read tool stays unchanged."""

    def __init__(self, root, *, held):
        self.read = Read(root=root)
        self.release = asyncio.Event()
        self.entered = asyncio.Event()
        self.agent_id = None
        self.room_id = None
        if not held:
            self.release.set()

    @property
    def definition(self):
        return self.read.definition

    async def handler(self, name, arguments):
        from roomkit.tools.context import current_tool_call

        call = current_tool_call()
        self.agent_id, self.room_id = call.channel_id, call.room_id
        self.entered.set()
        async with asyncio.timeout(300):
            await self.release.wait()
        return await self.read.handler(name, arguments)


class ParallelScenario:
    """Host-controlled assertions around real agent tools, rooms and model turns."""

    def __init__(self, kit, backend, scope, pauses, diagnostics):
        self.kit, self.backend, self.scope = kit, backend, scope
        self.pauses, self.diagnostics = pauses, diagnostics
        self.tasks = {}
        self.message = None
        self.followup = None
        self.marker = "PARALLEL_" + uuid4().hex[:12]

    async def start(self):
        if self.tasks or (await self.backend.status(self.scope))["tasks"]:
            raise RuntimeError("start the parallel scenario in a fresh demo session")
        receipts = await asyncio.gather(
            *(
                DelegateTaskTool(self.backend).call(
                    {"profile": profile, "task": TASK}, scope=self.scope
                )
                for profile in ("researcher", "reviewer")
            )
        )
        self.tasks = {item["data"]["profile"]: item["data"] for item in receipts}
        if len({item["agent_id"] for item in self.tasks.values()}) != 2:
            raise RuntimeError("parallel profiles did not create distinct instances")
        for profile, record in self.tasks.items():
            print(f"Host parallel: {profile} task={record['task_id']} agent={record['agent_id']}")

    def pause_for(self, profile):
        agent_id = self.tasks[profile]["agent_id"]
        return next(p for p in self.pauses if p.agent_id == agent_id)

    async def states(self):
        return {item["task_id"]: item for item in (await self.backend.status(self.scope))["tasks"]}

    async def run(self):
        await self.start()
        # Wait for actual tool entry, not a guessed inference duration.
        async with asyncio.timeout(180):
            await asyncio.gather(*(p.entered.wait() for p in self.pauses))
        if (
            len(self.pauses) != 2
            or self.pause_for("researcher").room_id == self.pause_for("reviewer").room_id
        ):
            raise RuntimeError("workers did not enter separate child rooms")
        initial = await self.states()
        if len(initial) != 2 or any(r["status"] != "running" for r in initial.values()):
            raise RuntimeError("both tasks must be running before the chat")
        print("Host parallel: both agents are running and paused at read.")

        async with asyncio.timeout(180):
            result = await self.kit.process_inbound(
                InboundMessage(
                    channel_id="user",
                    sender_id="developer",
                    content=TextContent(
                        body="Hello! Reply briefly to my greeting while the two workers continue. "
                        "Do not manage tasks or call tools for this greeting."
                    ),
                ),
                room_id=ROOM,
            )
        observed = [
            r
            for r in self.diagnostics.records
            if r["kind"] == "response" and r["channel_id"] == "assistant"
        ]
        if (
            result.error is not None
            or not observed
            or not observed[-1]["text_chars"]
            or observed[-1]["end_reason"] != "completed"
        ):
            raise RuntimeError("no completed principal reply while workers were paused")
        current = await self.states()
        if len(current) != 2 or any(r["status"] != "running" for r in current.values()):
            raise RuntimeError("ordinary chat changed the two running tasks")
        print("Host parallel: principal answered while both workers remained running.")

        researcher, reviewer = self.tasks["researcher"], self.tasks["reviewer"]
        self.message = (
            await AgentSendTool(self.backend).call(
                {
                    "agent_id": researcher["agent_id"],
                    "message": "Focus on exactly 50 units. Include the verification marker "
                    f"{self.marker} "
                    "in your report to acknowledge this clarification.",
                },
                scope=self.scope,
            )
        )["data"]
        if self.message["status"] != "queued" or not self.message["history_recorded"]:
            raise RuntimeError("researcher did not accept and retain the clarification")
        if self.message["task_id"] != researcher["task_id"]:
            raise RuntimeError("clarification targeted the wrong task")
        for profile in ("researcher", "reviewer"):
            events = await self.kit.store.list_events(self.pause_for(profile).room_id, limit=100)
            recorded = [
                e
                for e in events
                if e.metadata.get("agent_message_id") == self.message["message_id"]
            ]
            if len(recorded) != (1 if profile == "researcher" else 0):
                raise RuntimeError("clarification history crossed instance boundaries")
        print("Host parallel: clarification queued and recorded only in the researcher room.")

        cancelled = (
            await CancelTaskTool(self.backend).call(
                {"task_id": reviewer["task_id"]}, scope=self.scope
            )
        )["data"]
        current = await self.states()
        if not cancelled["cancelled"] or cancelled["task"]["status"] != "cancelled":
            raise RuntimeError("reviewer was not cancelled")
        if current[researcher["task_id"]]["status"] != "running":
            raise RuntimeError("reviewer cancellation affected the researcher")
        print("Host parallel: reviewer cancelled; researcher remains running.")

        self.pause_for("researcher").release.set()
        async with asyncio.timeout(180):
            completed = await self.backend.wait(researcher["task_id"])
        if (
            completed["status"] != "completed"
            or completed["delivery"] != "delivered"
            or self.marker not in (completed["result"] or "")
        ):
            raise RuntimeError("researcher report did not acknowledge the actual clarification")
        self.followup = (
            await AgentFollowupTool(self.backend).call(
                {
                    "agent_id": researcher["agent_id"],
                    "task": FOLLOWUP_TASK
                    + " Recall the verification marker from the earlier clarification.",
                },
                scope=self.scope,
            )
        )["data"]
        async with asyncio.timeout(180):
            resumed = await self.backend.wait(self.followup["task_id"])
        if (
            resumed["agent_id"] != researcher["agent_id"]
            or resumed["task_id"] == researcher["task_id"]
        ):
            raise RuntimeError("follow-up did not reuse the researcher with a new task")
        if (
            resumed["status"] != "completed"
            or resumed["delivery"] != "delivered"
            or self.marker not in (resumed["result"] or "")
        ):
            raise RuntimeError("follow-up did not retain the clarification or deliver its report")
        final = await self.states()
        if (
            len(final) != 3
            or final[reviewer["task_id"]]["status"] != "cancelled"
            or final[reviewer["task_id"]]["delivery"] != "suppressed"
        ):
            raise RuntimeError("unexpected task creation or cancelled-reviewer delivery")
        print(
            "Verified parallel: isolated clarification, independent cancellation, "
            "continued researcher with retained history and principal delivery."
        )
        return {"tasks": list(final.values()), "message": self.message, "marker": self.marker}


async def run(root, *, dynamic=False, live=False, chat=False, held=False, parallel=False):
    (root / "shipping.py").write_text(SOURCE)
    scope = AgentScope(ROOM, "assistant")
    worker_calls, resolutions, replies, pauses, handoffs = [], [], [], [], []
    provider_factory = live_provider if live or chat else ScriptedWorker
    held = held or parallel
    parallel_result = None

    def paused_read():
        pause = PausedRead(root, held=held)
        pauses.append(pause)
        return pause

    def blueprint(profile):
        return AgentConfig(
            provider_factory=provider_factory,
            system_prompt=(
                f"You are the {profile}. Use actual tool results and obey the supplied task. "
                "For each mission requiring a read, call read again, "
                "even with prior results in history. "
                "Read shipping.py before answering. code_outline accepts a regular file: use "
                "path=shipping.py, never the project directory. "
                "You have no mutation or delegation tools."
            ),
            tools_factory=(
                (lambda: [paused_read(), CodeOutline(root=root), Grep(root=root)])
                if profile == "researcher"
                else (lambda: [paused_read()])
            ),
            tool_timeout_seconds=330 if held else 30,
            tool_loop_timeout_seconds=450 if held else 180,
        )

    async def resolve(request):
        resolutions.append(request)
        print(f"Host resolver: {request.profile}, room={request.scope.room_id}")
        return blueprint(request.profile)

    async with RoomKit() as kit:
        await kit.create_room(room_id=ROOM)
        diagnostics = TurnDiagnostics(kit)
        options = (
            {"resolver": resolve, "allowed_profiles": ("researcher", "reviewer")}
            if dynamic
            else {"profiles": {name: blueprint(name) for name in ("researcher", "reviewer")}}
        )
        async with RoomKitAgentBackend(kit, scope=scope, **options) as backend:
            tools = [
                DelegateTask(DelegateTaskTool(backend)),
                TaskStatus(TaskStatusTool(backend)),
                CancelTask(CancelTaskTool(backend)),
                AgentFollowup(AgentFollowupTool(backend)),
                AgentSend(AgentSendTool(backend)),
            ]
            kit.register_channel(
                AIChannel(
                    "assistant",
                    provider=live_provider() if live or chat else ScriptedSpeaker(),
                    tools=tools,
                    system_prompt=(
                        "You are the principal assistant. For source analysis, actually call "
                        "delegate_task with profile researcher or reviewer and a complete task. "
                        "Each call starts a new isolated instance. researcher can outline, grep "
                        "and read; reviewer can only read. Keep chatting while they work. "
                        "After delegate_task, acknowledge its ID and end your turn. Do not poll "
                        "task_status to wait for completion: reports arrive automatically. "
                        "Use task_status for observed state and cancel_task when asked to stop. "
                        "agent_followup continues an idle agent_id with a new task_id and its "
                        "existing history. agent_send queues a clarification to a running agent; "
                        "queued does not mean already read. These take agent_id, not a profile. "
                        "Copy IDs verbatim from tool results. If an ID is unknown, compare it "
                        "with task_status and correct it; do not infer expiry or silently replace "
                        "a request for the same agent with a new delegation. "
                        "When a worker report arrives, explain it to the user. Never claim an "
                        "action without its tool result. Proposed tests have predicted outcomes, "
                        "not observed pass/fail results: these workers cannot execute tests. "
                        "Source file: shipping.py."
                    ),
                )
            )
            await kit.attach_channel(ROOM, "assistant", category=ChannelCategory.INTELLIGENCE)

            @kit.hook(HookTrigger.ON_TOOL_CALL, execution=HookExecution.ASYNC)
            async def observe(event, _context):
                if event.channel_id.startswith("agent-"):
                    active = [
                        r
                        for r in (await backend.status(scope))["tasks"]
                        if r["agent_id"] == event.channel_id and r["status"] == "running"
                    ]
                    worker_calls.append((active[-1]["task_id"] if active else None, event))
                    print(
                        f"Host tool: {active[-1]['task_id'] if active else 'unassigned'} "
                        f"{event.name} {json.dumps(event.arguments)} error={event.is_error}"
                    )
                    if event.is_error:
                        print(
                            f"Host: {event.channel_id} {event.name} failed; "
                            f"arguments={json.dumps(event.arguments)}; result={event.result}"
                        )

            @kit.hook(HookTrigger.AFTER_BROADCAST, execution=HookExecution.ASYNC)
            async def reply(event, _context):
                if event.source.channel_id == "assistant" and event.responds_to:
                    replies.append(event)
                if event.type == EventType.INSTRUCTION and event.metadata.get("task_id"):
                    handoffs.append(event)

            user = CLIChannel("user", prompt="You: ") if chat else WebSocketChannel("user")
            if not chat:

                async def display(_connection, event):
                    if isinstance(event.content, TextContent):
                        print("Assistant:", event.content.body)

                user.register_connection("console", display, room_id=ROOM)
            kit.register_channel(user)
            await kit.attach_channel(ROOM, "user")
            await kit.ensure_participant(ROOM, "user", "developer")

            async def check():
                records = (await backend.status(scope))["tasks"]
                if (root / "shipping.py").read_text() != SOURCE:
                    raise RuntimeError("source was modified")
                if not records:
                    print("Host: no tasks to verify.")
                    return
                pending = False
                for record in records:
                    print("Host:", json.dumps(record, ensure_ascii=False))
                    if record["status"] == "running" or record["delivery"] == "pending":
                        pending = True
                        continue
                    if record["status"] == "failed":
                        raise RuntimeError("worker failed")
                    if record["status"] != "completed":
                        continue
                    observed = [
                        e
                        for task_id, e in worker_calls
                        if task_id == record["task_id"]
                        and e.channel_id == record["agent_id"]
                        and e.name == "read"
                        and not e.is_error
                    ]
                    if not observed or record["delivery"] != "delivered":
                        raise RuntimeError(
                            f"{record['task_id']}: missing a fresh successful read "
                            f"(observed={len(observed)}) or principal delivery "
                            f"(delivery={record['delivery']})"
                        )
                    pages = [json.loads(event.result)["data"] for event in observed]
                    if not any(
                        page["path"] == "shipping.py"
                        and {"number": 3, "text": "    if subtotal > 50:"} in page["lines"]
                        for page in pages
                    ):
                        raise RuntimeError("worker did not actually read the faulty source line")
                    instructions = {
                        event.id
                        for event in handoffs
                        if event.metadata["task_id"] == record["task_id"]
                    }
                    if not any(
                        event.responds_to in instructions
                        and isinstance(event.content, TextContent)
                        and event.content.body.strip()
                        and event.metadata.get("loop_end_reason") == "completed"
                        for event in replies
                    ):
                        raise RuntimeError("missing principal reply linked to this task's report")
                print(
                    "Host: verification pending; source unchanged."
                    if pending
                    else "Host: source unchanged; tool evidence and linked replies checked."
                )

            scenario = ParallelScenario(kit, backend, scope, pauses, diagnostics)

            if chat:

                async def start(args):
                    try:
                        receipt = await tools[0].call(
                            {"profile": args.strip() or "researcher", "task": TASK}, scope=scope
                        )
                        print("Host:", json.dumps(receipt["data"]))
                    except ToolError as error:
                        print("Host:", error)

                async def status(_args):
                    for record in (await backend.status(scope))["tasks"]:
                        print(
                            "Host:",
                            record["task_id"],
                            record["profile"],
                            record["agent_id"],
                            record["status"],
                            record["delivery"],
                        )

                async def cancel(args):
                    records = (await backend.status(scope))["tasks"]
                    if not records:
                        print("Host: no tasks")
                        return
                    try:
                        print(
                            "Host:",
                            await backend.cancel(
                                scope, task_id=args.strip() or records[-1]["task_id"]
                            ),
                        )
                    except ToolError as error:
                        print("Host:", error)

                async def address(args, *, followup):
                    records = (await backend.status(scope))["tasks"]
                    if not records:
                        print("Host: no agent instances")
                        return
                    words = args.strip().split(maxsplit=1)
                    agent_id = records[-1]["agent_id"]
                    content = args.strip()
                    if words and words[0].startswith("agent-"):
                        agent_id, content = words[0], words[1] if len(words) > 1 else ""
                    tool = tools[3] if followup else tools[4]
                    arguments = {
                        "agent_id": agent_id,
                        "task" if followup else "message": content
                        or (FOLLOWUP_TASK if followup else "Focus on the case subtotal == 50."),
                    }
                    try:
                        print(
                            "Host:", json.dumps((await tool.call(arguments, scope=scope))["data"])
                        )
                    except ToolError as error:
                        print("Host:", error)

                async def followup(args):
                    await address(args, followup=True)

                async def send(args):
                    await address(args, followup=False)

                async def gate_action(args, *, release):
                    selected = [p for p in pauses if not args.strip() or p.agent_id == args.strip()]
                    if not selected:
                        print("Host: no matching read pause; wait for tool entry or use /list.")
                        return
                    for pause in selected:
                        if release:
                            pause.release.set()
                        else:
                            pause.release.clear()
                            pause.entered.clear()
                    print(
                        f"Host: {len(selected)} read pause(s) "
                        f"{'released' if release else 'armed for the next read'}."
                    )

                async def pause(args):
                    await gate_action(args, release=False)

                async def release(args):
                    await gate_action(args, release=True)

                async def start_parallel(_args):
                    if not held:
                        print("Host: restart with --hold for interactive parallel validation.")
                        return
                    try:
                        await scenario.start()
                    except (RuntimeError, ToolError) as error:
                        print("Host:", error)

                async def show_diagnostics(_args):
                    diagnostics.show()

                async def verify(_args):
                    try:
                        await check()
                    except RuntimeError as error:
                        print("Host:", error)

                await user.run(
                    kit,
                    room_id=ROOM,
                    sender_id="developer",
                    commands={
                        "/start": start,
                        "/parallel": start_parallel,
                        "/diagnostics": show_diagnostics,
                        "/followup": followup,
                        "/send": send,
                        "/pause": pause,
                        "/status": status,
                        "/list": status,
                        "/cancel": cancel,
                        "/release": release,
                        "/check": verify,
                    },
                    welcome=(
                        f"Configuration: {'dynamic resolver' if dynamic else 'static profiles'}.\n"
                        "researcher: read/grep/code_outline; reviewer: read only.\n"
                        "Ask the assistant to delegate an analysis of shipping.py.\n"
                        "/start researcher or /start reviewer: direct host launch.\n"
                        "/parallel: launch both profiles in a fresh session.\n"
                        "/followup [agent_id] [task]: continue an idle instance.\n"
                        "/send [agent_id] [message]: clarify a running mission.\n"
                        "/list, /cancel [task_id], /check; quit closes owned tasks.\n"
                        "/diagnostics: recent turn endings and errors (no prompt bodies).\n"
                        "Idle expiry: 5 minutes. /pause [agent_id] arms the next read.\n"
                        + (
                            "DEMO PAUSE: read waits for /release [agent_id] (omit ID for all)."
                            if held
                            else ""
                        )
                    ),
                )
            elif parallel:
                parallel_result = await scenario.run()
                await check()
                if len(pauses) != 2 or (dynamic and len(resolutions) != 2):
                    raise RuntimeError("follow-up unexpectedly resolved a new instance")
            else:
                for profile in ("researcher", "reviewer"):
                    before = len((await backend.status(scope))["tasks"])
                    async with asyncio.timeout(180):
                        await kit.process_inbound(
                            InboundMessage(
                                channel_id="user",
                                sender_id="developer",
                                content=TextContent(
                                    body=f"Start {profile} with delegate_task: {TASK}"
                                ),
                            ),
                            room_id=ROOM,
                        )
                        records = (await backend.status(scope))["tasks"]
                        if len(records) != before + 1:
                            raise RuntimeError("no fresh delegation was observed")
                        if records[-1]["profile"] != profile:
                            raise RuntimeError("the requested profile was not selected")
                        result = await backend.wait(records[-1]["task_id"])
                        if result["status"] != "completed":
                            raise RuntimeError("worker did not complete")
                first = records[0]
                async with asyncio.timeout(180):
                    await kit.process_inbound(
                        InboundMessage(
                            channel_id="user",
                            sender_id="developer",
                            content=TextContent(
                                body=(
                                    f"Continue {first['agent_id']} using agent_followup: "
                                    f"{FOLLOWUP_TASK}"
                                )
                            ),
                        ),
                        room_id=ROOM,
                    )
                    records = (await backend.status(scope))["tasks"]
                    if len(records) != 3 or records[-1]["agent_id"] != first["agent_id"]:
                        raise RuntimeError("expected a fresh mission on the first instance")
                    await backend.wait(records[-1]["task_id"])
                await check()
                if len({record["agent_id"] for record in records}) != 2:
                    raise RuntimeError("expected two distinct instances")
                if dynamic and len(resolutions) != 2:
                    raise RuntimeError("expected one resolution per launch")
                if not replies:
                    raise RuntimeError("missing principal response")
                print(
                    "Verified: two instances, three missions, same-agent continuation, "
                    "real reads and principal replies."
                )
    if (root / "shipping.py").read_text() != SOURCE:
        raise RuntimeError("source changed during shutdown")
    print("Closed: owned agent instances released; source unchanged.")
    return parallel_result


def cli(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--live", action="store_true")
    mode.add_argument("--chat", action="store_true")
    parser.add_argument("--dynamic", action="store_true")
    parser.add_argument("--hold", action="store_true")
    parser.add_argument("--parallel", action="store_true", help="Verify two concurrent agents")
    args = parser.parse_args(argv)
    if args.parallel and args.chat:
        parser.error("use /parallel inside --chat --hold, or --parallel for the automatic scenario")
    if args.hold and not args.chat:
        parser.error("--hold is only for interactive chat")
    with tempfile.TemporaryDirectory(prefix="roomkit-agents-") as temporary:
        print("Temporary project:", temporary)
        asyncio.run(
            run(
                Path(temporary),
                dynamic=args.dynamic,
                live=args.live,
                chat=args.chat,
                held=args.hold,
                parallel=args.parallel,
            )
        )


if __name__ == "__main__":
    cli()
