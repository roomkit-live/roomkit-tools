"""Locate a method with code_outline, read its range, fix it and run real tests.

The default model is scripted. --live uses OPENAI_BASE_URL / OPENAI_MODEL for
vLLM. All tools are native Rust; the host independently checks files and tests.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import shlex
import sys
import tempfile
from pathlib import Path

from roomkit import (
    AIChannel,
    ChannelCategory,
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
from roomkit_tools import (
    CodeOutline,
    CodeOutlineTool,
    Diff,
    DiffTool,
    Edit,
    EditTool,
    Read,
    ReadTool,
    Shell,
    ShellTool,
    Workspace,
)

SOURCE = '''class ShippingService:
    def label(self, order_id: int) -> str:
        return f"Order {order_id}"

    def calculate_fee(self, subtotal: int) -> int:
        """Shipping is free for orders of 50 units or more."""
        if subtotal > 50:
            return 0
        return 5

    def estimate_delivery(self, processing_days: int) -> int:
        return processing_days + 2


def service_name() -> str:
    return "Standard shipping"
'''
FIXED = SOURCE.replace("subtotal > 50", "subtotal >= 50")
TESTS = """import unittest
from shipping import ShippingService


class ShippingTests(unittest.TestCase):
    def test_below_threshold(self):
        self.assertEqual(ShippingService().calculate_fee(49), 5)

    def test_at_threshold(self):
        self.assertEqual(ShippingService().calculate_fee(50), 0)

    def test_above_threshold(self):
        self.assertEqual(ShippingService().calculate_fee(51), 0)
"""
TASK = (
    "Fix ShippingService.calculate_fee in shipping.py: shipping is free from 50 units. "
    "First run the tests to reproduce the failure. Call code_outline on shipping.py, "
    "locate calculate_fee, then read only that method using its start_line and end_line "
    "(offset=start_line, limit=end_line-start_line+1). Make one minimal edit with "
    "expected_revision from that read. Compare shipping.original.txt with shipping.py "
    "using diff, run the tests again and read the corrected method using the same range. "
    "Preserve the other methods, tests and original snapshot. Report only observed results."
)


def test_command():
    return shlex.join([sys.executable, "-B", "-m", "unittest", "-v", "test_shipping"])


def tool_results(context, name):
    return [
        json.loads(part.result)["data"]
        for message in context.messages
        if isinstance(message.content, list)
        for part in message.content
        if isinstance(part, AIToolResultPart) and part.name == name and not part.is_error
    ]


class OutlineProvider(MockAIProvider):
    """Carry actual outline ranges and read revisions into subsequent calls."""

    def __init__(self):
        steps = [
            ("shell", {"command": test_command()}),
            ("code_outline", {"path": "shipping.py"}),
            ("read", {"path": "shipping.py"}),
            (
                "edit",
                {"path": "shipping.py", "old_text": "subtotal > 50", "new_text": "subtotal >= 50"},
            ),
            ("diff", {"old_path": "shipping.original.txt", "new_path": "shipping.py"}),
            ("shell", {"command": test_command()}),
            ("read", {"path": "shipping.py"}),
        ]
        super().__init__(
            streaming=False,
            ai_responses=[
                AIResponse(
                    content="",
                    finish_reason="tool_calls",
                    tool_calls=[AIToolCall(id=f"step-{i}", name=name, arguments=arguments)],
                )
                for i, (name, arguments) in enumerate(steps)
            ]
            + [AIResponse(content="The selected method is fixed; all three tests now pass.")],
        )

    async def generate(self, context):
        response = await super().generate(context)
        for call in response.tool_calls or []:
            if call.name == "read":
                symbols = tool_results(context, "code_outline")[-1]["symbols"]
                method = next(s for s in symbols if s["name"] == "calculate_fee")
                call.arguments.update(
                    offset=method["start_line"], limit=method["end_line"] - method["start_line"] + 1
                )
            if call.name == "edit":
                call.arguments["expected_revision"] = tool_results(context, "read")[-1]["revision"]
        return response


def require(condition, message):
    if not condition:
        raise RuntimeError("Scenario incomplete: " + message)


def check_tests(result, fixed):
    require(result["exit_code"] == (0 if fixed else 1), "unexpected test exit code")
    require(not result["timed_out"] and result["output_complete"], "test output incomplete")
    stderr = result["stderr"]
    require(not stderr["truncated"] and "Ran 3 tests" in stderr["text"], "missing test run")
    require(
        ("\nOK\n" if fixed else "FAIL: test_at_threshold") in stderr["text"],
        "unexpected test outcome",
    )


async def verify(root, events):
    successful = [(e, json.loads(e.result)["data"]) for e in events if not e.is_error]
    edits = [
        (i, e) for i, (e, data) in enumerate(successful) if e.name == "edit" and data["changed"]
    ]
    require(len(edits) == 1, "expected one source edit")
    position, edit = edits[0]
    before, after = successful[:position], successful[position + 1 :]
    outlines = [d for e, d in before if e.name == "code_outline" and d["path"] == "shipping.py"]
    require(bool(outlines) and not outlines[-1]["syntax_errors"], "missing valid outline")
    method = next((s for s in outlines[-1]["symbols"] if s["name"] == "calculate_fee"), None)
    require(method is not None and method["kind"] == "method", "method was not found")
    start, end = method["start_line"], method["end_line"]

    def selected_read(rows, source):
        for event, data in rows:
            if event.name != "read" or data["path"] != "shipping.py":
                continue
            expected = [
                {"number": i, "text": text}
                for i, text in enumerate(source.splitlines(), 1)
                if start <= i <= end
            ]
            if (
                data["lines"] == expected
                and event.arguments.get("offset") == start
                and event.arguments.get("limit") == end - start + 1
            ):
                return data
        return None

    original = selected_read(before, SOURCE)
    require(original is not None, "original method was not read using the outline range")
    require(
        edit.arguments.get("expected_revision") == original["revision"], "edit lost read revision"
    )
    require(selected_read(after, FIXED) is not None, "corrected method was not read")
    for rows, fixed in [(before, False), (after, True)]:
        tests = [
            d
            for e, d in rows
            if e.name == "shell"
            and shlex.split(e.arguments["command"]) == shlex.split(test_command())
        ]
        require(bool(tests), "missing observed test command")
        check_tests(tests[-1], fixed)
    require(
        any(e.name == "diff" and d["additions"] == d["deletions"] == 1 for e, d in after),
        "missing one-line diff",
    )
    for name, text in {
        "shipping.py": FIXED,
        "shipping.original.txt": SOURCE,
        "test_shipping.py": TESTS,
    }.items():
        require((root / name).read_bytes() == text.encode(), "unexpected file content: " + name)
    shell = await ShellTool.create(root)
    check_tests((await shell.shell(test_command()))["data"], True)
    print(
        "Verified: outline selected the method, targeted reads and revision-checked edit succeeded."
    )
    print("One-line correction; three tests pass. Other source, tests and snapshot are unchanged.")


async def run(root, provider=None):
    require(not any(root.iterdir()), "the temporary project must be empty")
    for name, text in {
        "shipping.py": SOURCE,
        "shipping.original.txt": SOURCE,
        "test_shipping.py": TESTS,
    }.items():
        (root / name).write_bytes(text.encode())
    events = []
    workspace = Workspace(root)
    prompt = TASK + " Test command: " + test_command()
    async with RoomKit() as kit:
        kit.register_channel(
            AIChannel(
                "agent",
                provider=provider or OutlineProvider(),
                temperature=0,
                max_tool_rounds=16,
                tools=[
                    CodeOutline(await CodeOutlineTool.create(workspace)),
                    Read(await ReadTool.create(workspace)),
                    Edit(await EditTool.create(workspace)),
                    Diff(await DiffTool.create(workspace)),
                    Shell(await ShellTool.create(workspace)),
                ],
                system_prompt="Use the tools and report actual observations. " + prompt,
            )
        )
        await kit.create_room(room_id="outline-demo")
        await kit.attach_channel("outline-demo", "agent", category=ChannelCategory.INTELLIGENCE)

        @kit.hook(HookTrigger.ON_TOOL_CALL, execution=HookExecution.ASYNC)
        async def observe(event, _context):
            if event.result is not None:
                events.append(event)
                print(f"Tool {event.name} (error={event.is_error}): {event.result}")
            return HookResult.allow()

        async def show(_connection, event):
            if isinstance(event.content, TextContent):
                print("Assistant:", event.content.body)

        user = WebSocketChannel("user")
        user.register_connection("console", show, room_id="outline-demo")
        kit.register_channel(user)
        await kit.attach_channel("outline-demo", "user")
        await kit.process_inbound(
            InboundMessage(
                channel_id="user", sender_id="developer", content=TextContent(body=prompt)
            ),
            room_id="outline-demo",
        )
    await verify(root, events)
    return events


def cli(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--live", action="store_true", help="Use the configured vLLM agent")
    args = parser.parse_args(argv)
    provider = None
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
                max_tokens=4096,
            )
        )
    print("Live vLLM" if args.live else "Scripted model; real RoomKit and Rust tools")
    with tempfile.TemporaryDirectory(prefix="roomkit-outline-") as temporary:
        print("Temporary project:", temporary)
        try:
            asyncio.run(run(Path(temporary), provider))
        except RuntimeError as error:
            parser.exit(1, f"{error}\n")


if __name__ == "__main__":
    cli()
