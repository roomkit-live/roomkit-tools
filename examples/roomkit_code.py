"""Fix a real bug through RoomKit and five selected Rust tools.

Run with an installed roomkit-tools[roomkit] wheel. The default provider is
scripted; tool calls, file changes and test processes are real. --live runs
one vLLM turn; --chat uses the same project in an interactive vLLM chat.
Each run owns a fresh temporary directory and removes it on exit.
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
from typing import Any

from roomkit import (
    AIChannel,
    ChannelCategory,
    CLIChannel,
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
from roomkit.providers.ai.base import AIContext, AIToolResultPart
from roomkit_tools import (
    Diff,
    DiffTool,
    Edit,
    EditTool,
    Grep,
    GrepTool,
    Read,
    ReadTool,
    Shell,
    ShellTool,
    Workspace,
)

SOURCE = '''def shipping_fee(subtotal: int) -> int:
    """Shipping is free for orders of 50 units or more."""
    if subtotal > 50:
        return 0
    return 5
'''
FIXED = SOURCE.replace("subtotal > 50", "subtotal >= 50")
TESTS = """import unittest

from shipping import shipping_fee


class ShippingTests(unittest.TestCase):
    def test_below_threshold(self):
        self.assertEqual(shipping_fee(49), 5)

    def test_at_threshold(self):
        self.assertEqual(shipping_fee(50), 0)

    def test_above_threshold(self):
        self.assertEqual(shipping_fee(51), 0)
"""
TASK = (
    "Fix the shipping fee bug: orders of 50 units or more must have free shipping. "
    "First run the tests to reproduce the failure, then locate the function with grep "
    "and read its source. Make the smallest correction with edit, carrying the read "
    "revision as expected_revision. Compare shipping.original.txt with shipping.py "
    "using diff, rerun the tests and read the corrected file. Keep the tests and "
    "original snapshot unchanged. Report only observed results."
)


def test_command() -> str:
    """Use this interpreter, including paths with spaces; avoid stale bytecode."""
    return shlex.join([sys.executable, "-B", "-m", "unittest", "-v", "test_shipping"])


def prepare_project(root: Path) -> None:
    """Create fixtures only in a new, host-owned empty directory."""
    files = {
        "shipping.py": SOURCE,
        "shipping.original.txt": SOURCE,
        "test_shipping.py": TESTS,
        "README.md": f"# Shipping example\n\n{TASK}\n\nRun tests: `{test_command()}`\n",
    }
    if any(root.iterdir()):
        raise ValueError("The example requires an empty directory")
    for name, content in files.items():
        (root / name).write_text(content, encoding="utf-8")


class CodingProvider(MockAIProvider):
    """Choose a fixed sequence, carrying the actual Rust read revision into edit."""

    def __init__(self) -> None:
        steps = [
            ("shell", {"command": test_command()}),
            ("grep", {"pattern": "def shipping_fee", "include": "*.py", "context_after": 4}),
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
                    tool_calls=[AIToolCall(id=f"step-{index}", name=name, arguments=arguments)],
                )
                for index, (name, arguments) in enumerate(steps)
            ]
            + [AIResponse(content="The boundary condition is corrected; all three tests pass.")],
        )

    async def generate(self, context: AIContext) -> AIResponse:
        response = await super().generate(context)
        if response.tool_calls and response.tool_calls[0].name == "edit":
            reads = [
                part
                for message in context.messages
                if isinstance(message.content, list)
                for part in message.content
                if isinstance(part, AIToolResultPart) and part.name == "read" and not part.is_error
            ]
            response.tool_calls[0].arguments["expected_revision"] = json.loads(reads[-1].result)[
                "data"
            ]["revision"]
        return response


def live_provider() -> AIProvider:
    from roomkit.providers.vllm import VLLMConfig, create_vllm_provider

    if not os.getenv("OPENAI_BASE_URL") or not os.getenv("OPENAI_MODEL"):
        raise SystemExit("Set OPENAI_BASE_URL (ending in /v1) and OPENAI_MODEL for live mode.")
    return create_vllm_provider(
        VLLMConfig(
            base_url=os.environ["OPENAI_BASE_URL"],
            model=os.environ["OPENAI_MODEL"],
            api_key=os.getenv("OPENAI_API_KEY") or "none",
            timeout=120.0,
            max_tokens=4096,
        )
    )


def require(condition: bool, message: str) -> None:
    """Keep scenario verification active even when Python runs with -O."""
    if not condition:
        raise RuntimeError(f"Scenario incomplete: {message}")


def check_test_result(data: dict[str, Any], *, fixed: bool) -> None:
    require(not data["timed_out"] and data["output_complete"], "test capture did not finish")
    require(data["signal"] is None, "test process ended by signal")
    require(data["exit_code"] == (0 if fixed else 1), "unexpected test exit code")
    stderr = data["stderr"]
    require(not stderr["truncated"] and "Ran 3 tests" in stderr["text"], "missing test run")
    marker = "\nOK\n" if fixed else "FAIL: test_at_threshold"
    require(marker in stderr["text"], "expected test outcome was not observed")


def verify_trace(events: list[ToolCallEvent], root: Path) -> None:
    """Check real before/after evidence, allowing equivalent paths and discovery order."""
    successful = [
        (event, json.loads(event.result)["data"]) for event in events if not event.is_error
    ]

    def path_is(argument: Any, name: str) -> bool:
        if not isinstance(argument, str):
            return False
        path = Path(argument)
        # Native tools already checked access. Normalize only for comparing
        # relative/absolute spellings in this host-created, symlink-free fixture.
        return os.path.normpath(path if path.is_absolute() else root / path) == str(root / name)

    def is_test(event: ToolCallEvent, data: dict[str, Any]) -> bool:
        if event.name != "shell" or data.get("cwd") != str(root):
            return False
        try:
            return shlex.split(event.arguments.get("command", "")) == shlex.split(test_command())
        except ValueError:
            return False

    def source_text(data: dict[str, Any]) -> str:
        return "\n".join(line["text"] for line in data["lines"]) + "\n"

    mutations = [
        (index, event, data)
        for index, (event, data) in enumerate(successful)
        if event.name == "edit"
        and path_is(event.arguments.get("path"), "shipping.py")
        and data["changed"]
    ]
    require(len(mutations) == 1, "expected one source correction")
    index, edit, changed = mutations[0]
    require(changed["replacements"] == 1, "expected one replacement")
    before, after = successful[:index], successful[index + 1 :]

    baseline = [data for event, data in before if is_test(event, data)]
    require(bool(baseline), "missing baseline shell test run")
    check_test_result(baseline[-1], fixed=False)
    require(
        any(
            event.name == "grep"
            and any(
                match["path"] == "shipping.py" and "def shipping_fee" in match["text"]
                for match in data["matches"]
            )
            for event, data in before
        ),
        "grep did not locate the function before editing",
    )
    reads = [
        data
        for event, data in before
        if event.name == "read" and path_is(event.arguments.get("path"), "shipping.py")
    ]
    require(bool(reads) and source_text(reads[-1]) == SOURCE, "original source was not read")
    require(
        edit.arguments.get("expected_revision") == reads[-1]["revision"], "edit lost the revision"
    )

    require(
        any(
            event.name == "diff"
            and path_is(event.arguments.get("old_path"), "shipping.original.txt")
            and path_is(event.arguments.get("new_path"), "shipping.py")
            and data["changed"]
            and data["additions"] == data["deletions"] == 1
            for event, data in after
        ),
        "missing one-line diff after editing",
    )
    passing = [data for event, data in after if is_test(event, data)]
    require(bool(passing), "missing passing shell test run")
    check_test_result(passing[-1], fixed=True)
    require(
        any(
            event.name == "read"
            and path_is(event.arguments.get("path"), "shipping.py")
            and source_text(data) == FIXED
            for event, data in after
        ),
        "corrected source was not read",
    )


def verify_files(root: Path) -> None:
    require(
        (root / "test_shipping.py").read_bytes() == TESTS.encode(), "test_shipping.py was changed"
    )
    require(
        (root / "shipping.original.txt").read_bytes() == SOURCE.encode(), "snapshot was changed"
    )
    require((root / "shipping.py").read_bytes() == FIXED.encode(), "source correction is missing")


async def verify(root: Path, events: list[ToolCallEvent]) -> None:
    """Check source integrity, tool evidence and an independent test execution."""
    await asyncio.to_thread(verify_files, root)
    verify_trace(events, root)
    shell = await ShellTool.create(root)
    check_test_result((await shell.shell(test_command()))["data"], fixed=True)
    print("Verified: failing test reproduced, minimal correction reviewed, three tests now pass.")
    print("Test file and original snapshot are unchanged; independent host verification passed.")


async def run(
    root: Path, provider: AIProvider, *, chat: bool = False, verbose: bool = False
) -> list[ToolCallEvent]:
    """Run the selected provider, then independently verify trace, files and tests."""
    workspace = Workspace(root)
    agent = AIChannel(
        "agent",
        provider=provider,
        temperature=0,
        max_tool_rounds=16,
        tools=[
            Grep(await GrepTool.create(workspace)),
            Read(await ReadTool.create(workspace)),
            Edit(await EditTool.create(workspace)),
            Diff(await DiffTool.create(workspace)),
            Shell(await ShellTool.create(workspace)),
        ],
        system_prompt=(
            f"Your project root is {root}; all four fixture files are directly inside it. "
            "Tool paths are relative to this root. Shell already starts there; "
            "do not cd elsewhere. "
            "The Python interpreter path is an executable, not the project directory. "
            f"{TASK} The exact test command is {test_command()}. "
            "Execute the requested tools in this turn, then explain their actual results. "
            "Do not simulate calls or infer a test result without running it. "
            "The scenario requires grep before the edit, even when the file name is known. "
            "Answer in the user's language."
        ),
    )
    events: list[ToolCallEvent] = []
    async with RoomKit() as kit:
        kit.register_channel(agent)
        await kit.create_room(room_id="code-demo")
        await kit.attach_channel("code-demo", "agent", category=ChannelCategory.INTELLIGENCE)

        @kit.hook(HookTrigger.ON_TOOL_CALL, execution=HookExecution.ASYNC)
        async def observe(event: ToolCallEvent, _context: RoomContext) -> HookResult:
            if event.result is not None:
                events.append(event)
                if not chat or verbose:
                    print(
                        f"Tool {event.name} {json.dumps(event.arguments)} "
                        f"(error={event.is_error}): {event.result}"
                    )
            return HookResult.allow()

        if chat:
            cli = CLIChannel("user", prompt="You: ")
            kit.register_channel(cli)
            await kit.attach_channel("code-demo", "user")

            async def start(_arguments: str) -> None:
                print("Starting one repair request. Waiting for the agent to finish...")
                await kit.process_inbound(
                    InboundMessage(
                        channel_id="user", sender_id="cli-user", content=TextContent(body=TASK)
                    ),
                    room_id="code-demo",
                )
                print("Agent turn finished. Type /check for independent verification, or quit.")

            async def check(_arguments: str) -> None:
                try:
                    await verify(root, events)
                except RuntimeError as error:
                    print(str(error))
                    print(
                        "The source can be correct while the required tool scenario is incomplete."
                    )

            await cli.run(
                kit,
                room_id="code-demo",
                commands={"/start": start, "/check": check},
                welcome=(
                    f"Temporary project: {root}\n"
                    "Type /start once to send the complete repair task as a single message.\n"
                    "Type /check to verify actual results, or quit to verify and clean up.\n"
                    "Ordinary input is sent one line at a time; do not paste a multiline task."
                ),
            )
        else:
            user = WebSocketChannel("user")

            async def show_reply(_connection: str, event: RoomEvent) -> None:
                if isinstance(event.content, TextContent):
                    print(f"Assistant: {event.content.body}")

            user.register_connection("console", show_reply, room_id="code-demo")
            kit.register_channel(user)
            await kit.attach_channel("code-demo", "user")
            await kit.process_inbound(
                InboundMessage(
                    channel_id="user",
                    sender_id="developer",
                    content=TextContent(body=TASK),
                ),
                room_id="code-demo",
            )

    await verify(root, events)
    return events


def cli(argv: list[str] | None = None) -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--live", action="store_true", help="Run one vLLM agent turn")
    mode.add_argument("--chat", action="store_true", help="Open an interactive vLLM chat")
    parser.add_argument(
        "--verbose", action="store_true", help="Also print full tool results in chat"
    )
    args = parser.parse_args(argv)
    provider = live_provider() if args.live or args.chat else CodingProvider()
    with tempfile.TemporaryDirectory(prefix="roomkit-code-") as directory:
        root = Path(directory)
        prepare_project(root)
        print(
            "Live vLLM" if args.live or args.chat else "Scripted model; real RoomKit and Rust tools"
        )
        try:
            asyncio.run(run(root, provider, chat=args.chat, verbose=args.verbose))
        except RuntimeError as error:
            parser.exit(1, f"{error}\n")


if __name__ == "__main__":
    cli()
