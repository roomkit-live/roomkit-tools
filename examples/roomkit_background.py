"""Keep chatting while a RoomKit worker inspects code with Rust tools.

Default: scripted models, real tools, completion and cancellation checks.
--live: the same automatic checks with vLLM. --chat: interactive vLLM chat.
--hold: explicitly pause code_outline until /release, useful for trying /cancel.
The worker profile is registered once; RoomKit creates a new room per task.
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
    CLIChannel,
    HookExecution,
    HookResult,
    HookTrigger,
    InboundMessage,
    RoomKit,
    TextContent,
    WebSocketChannel,
)
from roomkit.providers.ai import AIResponse, AIToolCall, MockAIProvider
from roomkit.providers.ai.base import AIToolResultPart
from roomkit.tools.context import current_tool_room_id
from roomkit_tools import CodeOutline, Grep, Read, Workspace

ROOM = "background-demo"
SOURCE = '''class ShippingService:
    def label(self, order_id: int) -> str:
        return f"Order {order_id}"

    def calculate_fee(self, subtotal: int) -> int:
        """Shipping is free for orders of 50 units or more."""
        if subtotal > 50:
            return 0
        return 5
'''
TASK = (
    "Analyze ShippingService.calculate_fee in shipping.py without modifying any file. "
    "Call code_outline, locate calculate_fee, and read only its start_line to end_line "
    "using offset and limit. Use grep to locate the threshold comparison. "
    "Explain the boundary bug and suggest the smallest correction, citing actual source lines. "
    "Do not claim to have edited anything or run tests."
)
START = "Call work_in_background exactly once with this task, then end your turn: " + TASK
STATUS = "Call task_status and report the observed state of this conversation's tasks."
CHAT = "While the analysis runs, answer this separate question: what is a Python docstring?"


def require(condition, message):
    if not condition:
        raise RuntimeError("Scenario incomplete: " + message)


def results(context, name):
    return [
        json.loads(part.result)
        for message in context.messages
        if isinstance(message.content, list)
        for part in message.content
        if isinstance(part, AIToolResultPart) and part.name == name and not part.is_error
    ]


def call(name, arguments):
    return AIResponse(
        content="",
        finish_reason="tool_calls",
        tool_calls=[AIToolCall(id=f"call-{name}", name=name, arguments=arguments)],
    )


class SpeakerProvider(MockAIProvider):
    """Choose tools from the current request, including actual returned task ids."""

    async def generate(self, context):
        self.calls.append(context)
        last = context.messages[-1]
        if last.role == "tool":
            return AIResponse(content="Observed tool result: " + str(last.content[0].result))
        text = str(last.content)
        if text.startswith("Call work_in_background"):
            return call("work_in_background", {"task": TASK})
        if text.startswith("Call task_status"):
            return call("task_status", {})
        if text.startswith("Call cancel_task with "):
            return call("cancel_task", json.JSONDecoder().raw_decode(text.split(" with ", 1)[1])[0])
        return AIResponse(content="A docstring documents a Python module, class or function.")


class WorkerProvider(MockAIProvider):
    """Use the actual outline to select the method, and quote the observed source."""

    async def generate(self, context):
        self.calls.append(context)
        outlines = results(context, "code_outline")
        if not outlines:
            return call("code_outline", {"path": "shipping.py"})
        method = next(s for s in outlines[-1]["data"]["symbols"] if s["name"] == "calculate_fee")
        reads = results(context, "read")
        if not reads:
            return call(
                "read",
                {
                    "path": "shipping.py",
                    "offset": method["start_line"],
                    "limit": method["end_line"] - method["start_line"] + 1,
                },
            )
        if not results(context, "grep"):
            return call("grep", {"pattern": "subtotal > 50", "path": "shipping.py"})
        line = next(line for line in reads[-1]["data"]["lines"] if "subtotal >" in line["text"])
        return AIResponse(
            content=(
                f"shipping.py:{line['number']}: {line['text'].strip()}. "
                "calculate_fee excludes exactly 50. Suggested correction: >= 50. "
                "Files were only read; no tests were run."
            )
        )


class OutlineGate:
    """Demo-only pause around a real tool; cancellation reaches this pending call."""

    def __init__(self, tool, held, changed):
        self.tool = tool
        self.held = held
        self.changed = changed
        self.entered = set()
        self.cancelled = set()
        self.releases = {}

    @property
    def definition(self):
        return self.tool.definition

    async def handler(self, name, arguments):
        room = current_tool_room_id()
        release = self.releases.setdefault(room, asyncio.Event())
        self.entered.add(room)
        self.changed.set()
        try:
            if self.held:
                await release.wait()
            return await self.tool.handler(name, arguments)
        except asyncio.CancelledError:
            self.cancelled.add(room)
            self.changed.set()
            raise

    def release(self, room):
        self.releases.setdefault(room, asyncio.Event()).set()


class BackgroundDemo:
    """Example wiring and observed evidence, not a reusable task scheduler."""

    def __init__(self, root, speaker=None, worker=None, *, held=False, chat=False):
        self.root = root
        self.speaker = speaker or SpeakerProvider()
        self.worker = worker or WorkerProvider()
        self.chat = chat
        self.kit = RoomKit()
        self.tools = []
        self.tasks = {}
        self.posts = {}
        self.answers = []
        self.turns = []
        self.reservation = None
        self.closing = False
        self.changed = asyncio.Event()
        workspace = Workspace(root)
        self.gate = OutlineGate(CodeOutline(workspace), held, self.changed)
        self.worker_tools = [self.gate, Grep(workspace), Read(workspace)]

    async def open(self):
        require(not any(self.root.iterdir()), "the temporary project must be empty")
        (self.root / "shipping.py").write_text(SOURCE)
        assistant = AIChannel(
            "assistant",
            provider=self.speaker,
            temperature=0,
            max_tool_rounds=8,
            system_prompt=(
                "You are a coding assistant with a separate read-only research worker. "
                "When asked to start background work, call work_in_background exactly once, "
                "briefly acknowledge its actual result and end your turn. "
                "The worker's report will be posted automatically; do not poll for it. "
                "Answer unrelated questions while it works. Use task_status or cancel_task "
                "when requested. You have no filesystem tools. Never invent a task id or "
                "claim a tool ran without its result. Answer in the user's language."
            ),
        )
        researcher = AIChannel(
            "researcher",
            provider=self.worker,
            temperature=0,
            max_tool_rounds=10,
            tool_loop_timeout_seconds=600,
            # A model may request outline and grep in the same round. Queued
            # companion calls must have the same allowance as the demo pause.
            tool_timeout_seconds=300 if self.gate.held else 30,
            tools=self.worker_tools,
            system_prompt=(
                f"Analyze the temporary project at {self.root}. Paths are relative to it. "
                "Use the actual read-only tools. Do not delegate or write files. "
                "Return a short report with file and line references. " + TASK
            ),
        )
        for channel in (assistant, researcher):
            self.kit.register_channel(channel)
        # These three tools belong to RoomKit. The worker's code tools are Rust.
        self.kit.setup_background(assistant, worker="researcher", hand_back="post")
        self.kit.setup_task_tools(assistant)
        await self.kit.create_room(room_id=ROOM)
        await self.kit.attach_channel(ROOM, "assistant", category=ChannelCategory.INTELLIGENCE)

        @self.kit.hook(HookTrigger.BEFORE_TOOL_USE)
        async def capacity(event, _context):
            if event.name == "work_in_background":
                # Reservation happens without yielding, before the tool can start a task.
                if self.closing or self.reservation is not None:
                    return HookResult.block(
                        "This demo runs one task at a time; finish or cancel it."
                    )
                if len(self.tasks) >= 8:
                    return HookResult.block("This demo allows eight tasks; restart for more.")
                self.reservation = event.tool_call_id
            return HookResult.allow()

        @self.kit.hook(HookTrigger.ON_TOOL_CALL, execution=HookExecution.ASYNC)
        async def tool(event, _context):
            if event.result is not None:
                self.tools.append(event)
                if not self.chat:
                    print(
                        f"Tool {event.channel_id}/{event.name} "
                        f"(error={event.is_error}): {event.result}"
                    )
                if (
                    event.name == "work_in_background"
                    and not event.refused
                    and event.tool_call_id == self.reservation
                ):
                    receipt = json.loads(event.result)
                    if event.is_error or receipt.get("status") != "started":
                        self.reservation = None
                self.changed.set()
            return HookResult.allow()

        @self.kit.hook(HookTrigger.ON_TASK_DELEGATED, execution=HookExecution.ASYNC)
        async def started(event, _context):
            meta = dict(event.metadata)
            self.tasks[meta["task_id"]] = {**meta, "end": None}
            self.changed.set()
            if not self.chat:
                print(f"Task started: {meta['task_id']} -> {meta['child_room_id']}")

        @self.kit.hook(HookTrigger.ON_TASK_COMPLETED, execution=HookExecution.ASYNC)
        async def finished(event, _context):
            meta = dict(event.metadata)
            self.tasks[meta["task_id"]]["end"] = meta["task_status"]
            self.reservation = None
            self.changed.set()
            if not self.chat:
                print(f"Task ended: {meta['task_id']} -> {meta['task_status']}")

        @self.kit.hook(HookTrigger.AFTER_BROADCAST, execution=HookExecution.ASYNC)
        async def posted(event, _context):
            if event.room_id == ROOM and event.metadata.get("task_id"):
                self.posts[event.id] = event
                self.changed.set()
            elif (
                event.room_id == ROOM
                and event.source.channel_id == "assistant"
                and isinstance(event.content, TextContent)
                and event.content.body.strip()
            ):
                self.answers.append(event)

        if self.chat:
            self.user = CLIChannel("user", prompt="You: ")
        else:
            self.user = WebSocketChannel("user")

            async def show(_connection, event):
                if isinstance(event.content, TextContent):
                    print("Assistant:", event.content.body)

            self.user.register_connection("console", show, room_id=ROOM)
        self.kit.register_channel(self.user)
        await self.kit.attach_channel(ROOM, "user")
        return self

    async def say(self, text):
        # A bounded host wait; model/tool deadlines are configured separately.
        async with asyncio.timeout(180):
            await self.kit.process_inbound(
                InboundMessage(
                    channel_id="user", sender_id="developer", content=TextContent(body=text)
                ),
                room_id=ROOM,
            )
        self.turns.append(text)

    async def until(self, predicate, description):
        async with asyncio.timeout(180):
            while not predicate():
                self.changed.clear()
                await self.changed.wait()
        require(predicate(), description)

    def latest(self):
        require(bool(self.tasks), "the agent has not started a task")
        return next(reversed(self.tasks.values()))

    def cancel_request(self):
        return "Call cancel_task with " + json.dumps({"task_id": self.latest()["task_id"]})

    def verify_files(self):
        require(sorted(p.name for p in self.root.iterdir()) == ["shipping.py"], "unexpected files")
        require((self.root / "shipping.py").read_bytes() == SOURCE.encode(), "source changed")

    def verify_task(self, task):
        self.verify_files()
        require(
            task["end"] in {"completed", "cancelled"}, "task has not completed or been cancelled"
        )
        posts = [e for e in self.posts.values() if e.metadata["task_id"] == task["task_id"]]
        events = [e for e in self.tools if e.room_id == task["child_room_id"]]
        require(all(e.channel_id == "researcher" for e in events), "unexpected worker")
        if task["end"] == "cancelled":
            require(not posts, "cancelled tool task posted a late result")
            return
        require(len(posts) == 1, "expected exactly one result in the parent room")
        successful = {e.name: json.loads(e.result)["data"] for e in events if not e.is_error}
        require(
            {"code_outline", "read", "grep"} <= successful.keys(), "missing worker tool evidence"
        )
        outline = successful["code_outline"]
        require(
            outline["path"] == str(self.root / "shipping.py") and not outline["syntax_errors"],
            "unexpected outline",
        )
        method = next(s for s in outline["symbols"] if s["name"] == "calculate_fee")
        read = successful["read"]
        expected = [
            {"number": i, "text": line}
            for i, line in enumerate(SOURCE.splitlines(), 1)
            if method["start_line"] <= i <= method["end_line"]
        ]
        require(
            read["lines"] == expected and read["revision"] == outline["revision"],
            "method was not read with its actual range/revision",
        )
        require(
            any(
                m["path"] == "shipping.py" and "subtotal > 50" in m["text"]
                for m in successful["grep"]["matches"]
            ),
            "missing comparison in grep",
        )
        require(
            "calculate_fee" in posts[0].content.body and "50" in posts[0].content.body,
            "report does not identify the method and boundary",
        )

    async def close(self):
        self.closing = True
        await self.kit.close()
        self.verify_files()


async def automatic(root, speaker=None, worker=None):
    """Observe completion and cancellation, using gates rather than timing guesses."""
    demo = BackgroundDemo(root, speaker, worker, held=True)
    try:
        await demo.open()
        for cancelled in (False, True):
            await demo.say(START)
            task = demo.latest()
            child = task["child_room_id"]
            await demo.until(
                lambda child=child: child in demo.gate.entered, "worker never reached code_outline"
            )
            require(task["end"] is None, "worker ended before the parallel chat")
            answers_before = len(demo.answers)
            await demo.say(CHAT)
            require(len(demo.answers) > answers_before, "no actual reply to the separate question")
            require(task["end"] is None, "chat did not complete while the worker was pending")
            await demo.say(STATUS)
            statuses = [
                json.loads(e.result)
                for e in demo.tools
                if e.name == "task_status" and not e.is_error
            ]
            require(
                statuses
                and any(
                    t["task_id"] == task["task_id"] and t["status"] == "running"
                    for t in statuses[-1]["tasks"]
                ),
                "running status not observed",
            )
            if cancelled:
                await demo.say(demo.cancel_request())
                await demo.until(
                    lambda task=task: task["end"] == "cancelled", "cancellation missing"
                )
                require(child in demo.gate.cancelled, "pending tool was not cancelled")
                require(
                    not any(
                        e.room_id == child and e.name == "code_outline" and not e.is_error
                        for e in demo.tools
                    ),
                    "held outline unexpectedly completed",
                )
            else:
                demo.gate.release(child)
                await demo.until(
                    lambda task=task: task["end"] is not None and bool(demo.posts),
                    "worker result not delivered",
                )
            demo.verify_task(task)
        require(
            len({t["child_room_id"] for t in demo.tasks.values()}) == 2,
            "requests did not get separate child rooms",
        )
        print("Verified: chat and task_status answered while a worker tool was pending.")
        print("One report delivered; second task cancelled, pending call stopped, no late report.")
        print("Two distinct child rooms; source unchanged. No edits or tests claimed.")
        return demo
    finally:
        await demo.close()


async def interactive(root, speaker, worker, held):
    demo = BackgroundDemo(root, speaker, worker, held=held, chat=True)
    try:
        await demo.open()

        async def request_tool(text, name):
            previous = len(demo.tools)
            await demo.say(text)
            calls = [e for e in demo.tools[previous:] if e.name == name and not e.is_error]
            if not calls:
                print(
                    f"Host check: no successful {name} call was observed. "
                    "The assistant's words alone do not confirm that action; try again."
                )
            return calls

        async def start(_args):
            calls = await request_tool(START, "work_in_background")
            if calls:
                receipt = json.loads(calls[-1].result)
                print(
                    f"Host check: task_id={receipt.get('task_id')}, status={receipt.get('status')}"
                )

        async def status(_args):
            await request_tool(STATUS, "task_status")

        async def cancel(_args):
            if not demo.tasks:
                print("No task has been started.")
                return
            calls = await request_tool(demo.cancel_request(), "cancel_task")
            if calls:
                print(
                    "Host check: cancellation response = "
                    + str(json.loads(calls[-1].result).get("status"))
                )

        async def release(_args):
            if not demo.tasks:
                print("No task has been started.")
                return
            demo.gate.release(demo.latest()["child_room_id"])
            print("Demo pause released; the worker continues. You can keep chatting.")

        async def check(_args):
            try:
                task = demo.latest()
                demo.verify_task(task)
                names = [
                    e.name
                    for e in demo.tools
                    if e.room_id == task["child_room_id"] and not e.is_error
                ]
                print(f"Task {task['task_id']}: {task['end']}; child room {task['child_room_id']}")
                print("Observed successful worker tools:", ", ".join(names) or "none")
                print("Verified: actual worker evidence and result delivery; source unchanged.")
            except RuntimeError as error:
                print(error)

        await demo.user.run(
            demo.kit,
            room_id=ROOM,
            sender_id="developer",
            commands={
                "/start": start,
                "/status": status,
                "/cancel": cancel,
                "/release": release,
                "/check": check,
            },
            welcome=(
                f"Temporary project: {root}\n"
                "An assistant talks to you; researcher has only code_outline, grep and read.\n"
                "/start: request an analysis. Chat normally while it works.\n"
                "/status: ask for task state. /cancel: ask to cancel the latest task.\n"
                "/check: verify evidence. quit: close tasks and remove the temporary project.\n"
                + (
                    "DEMO PAUSE ON: code_outline waits for /release (up to 300 seconds).\n"
                    "Try /start, a normal question, /status, then /release or /cancel."
                    if held
                    else "Tasks run immediately; their reports arrive automatically."
                )
            ),
        )
    finally:
        await demo.close()
    print("Closed: task cleanup finished; source unchanged.")
    return demo


def live_provider():
    from roomkit.providers.vllm import VLLMConfig, create_vllm_provider

    require(
        os.getenv("OPENAI_BASE_URL") and os.getenv("OPENAI_MODEL"),
        "set OPENAI_BASE_URL and OPENAI_MODEL for vLLM",
    )
    return create_vllm_provider(
        VLLMConfig(
            base_url=os.environ["OPENAI_BASE_URL"],
            model=os.environ["OPENAI_MODEL"],
            api_key=os.getenv("OPENAI_API_KEY") or "none",
            timeout=120,
            max_tokens=2048,
        )
    )


def cli(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--live", action="store_true", help="Run automatic checks with vLLM")
    mode.add_argument("--chat", action="store_true", help="Interactive vLLM chat")
    parser.add_argument("--hold", action="store_true", help="Pause worker outline calls in chat")
    args = parser.parse_args(argv)
    if args.hold and not args.chat:
        parser.error("--hold is for --chat; automatic checks already control their pauses")
    with tempfile.TemporaryDirectory(prefix="roomkit-background-") as temporary:
        root = Path(temporary)
        print(
            "Live vLLM"
            if args.live or args.chat
            else "Scripted models; real RoomKit and Rust tools"
        )
        print("Temporary project:", root)
        try:
            speaker, worker = (
                (live_provider(), live_provider()) if args.live or args.chat else (None, None)
            )
            asyncio.run(
                interactive(root, speaker, worker, args.hold)
                if args.chat
                else automatic(root, speaker, worker)
            )
        except (RuntimeError, TimeoutError) as error:
            parser.exit(1, f"Background demo failed: {error or 'timed out'}\n")


if __name__ == "__main__":
    cli()
