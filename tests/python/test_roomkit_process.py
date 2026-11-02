"""Persistent jobs selected individually and used across RoomKit agent turns."""

from __future__ import annotations

import asyncio
import json
import tempfile
import unittest
from pathlib import Path

from roomkit import (
    AIChannel,
    HookExecution,
    HookResult,
    HookTrigger,
    InboundMessage,
    RoomKit,
    ToolCallEvent,
    WebSocketChannel,
)
from roomkit.core.exceptions import ToolFailedError, ToolRefusedError, UnservedToolCallError
from roomkit.models.channel import ChannelBinding
from roomkit.models.context import RoomContext
from roomkit.models.enums import ChannelCategory, ChannelType
from roomkit.models.event import EventSource, RoomEvent, TextContent
from roomkit.models.room import Room
from roomkit.providers.ai.base import AIContext, AIResponse, AIToolCall, AIToolResultPart
from roomkit.providers.ai.mock import MockAIProvider
from roomkit.tools.compose import extract_tools
from roomkit_helpers import in_call
from roomkit_tools import (
    InvalidInputError,
    OutputRead,
    OutputStore,
    ProcessRead,
    ProcessSession,
    ProcessStart,
    ProcessStatus,
    ProcessStop,
    ProcessWrite,
    Shell,
    ShellTool,
    ToolIOError,
    roomkit_owner,
)
from roomkit_tools.roomkit.commands import _roomkit_owner

ADAPTERS = (ProcessStart, ProcessStatus, ProcessRead, ProcessWrite, ProcessStop)
OWNER_FIXTURE = Path(__file__).resolve().parents[1] / "fixtures/roomkit_owner.json"


async def wait_job(session: ProcessSession, room: str, agent: str, pid: str, **expected):
    """Wait until `agent`'s job in `room` reports the expected status fields."""
    async with asyncio.timeout(5):
        while True:
            with in_call(room, agent):
                result = await ProcessStatus(session).handler("process_status", {"process_id": pid})
            status = json.loads(result)["data"]["processes"][0]
            if all(status[key] == value for key, value in expected.items()):
                return status
            await asyncio.sleep(0.005)


class RoomKitProcessTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self) -> None:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)

    async def test_individual_selection_and_rust_schemas(self) -> None:
        async with ProcessSession(self.root) as session:
            for adapter in ADAPTERS:
                tool = adapter(session)
                definitions, handler = extract_tools([tool])
                self.assertEqual(len(definitions), 1)
                name = definitions[0].name
                self.assertEqual(
                    definitions[0].parameters, session.definition(name)["input_schema"]
                )
                self.assertNotIn("owner", definitions[0].parameters["properties"])
                with self.assertRaises(UnservedToolCallError):
                    await handler("read", {"path": "notes"})

    async def test_failures_and_closed_session_map_to_roomkit_errors(self) -> None:
        session = ProcessSession(self.root, max_processes=1, max_processes_per_owner=1)
        _, handler = extract_tools([adapter(session) for adapter in ADAPTERS])
        with in_call("room", "agent"):
            async with session:
                for name, arguments in [
                    ("process_start", {"command": ""}),
                    ("process_start", {"command": "pwd", "owner": "other"}),
                    ("process_start", {"command": "pwd", "cwd": ".."}),
                    ("process_status", {"process_id": "missing"}),
                    ("process_read", {"process_id": "missing"}),
                    ("process_write", {"process_id": "missing", "text": "x"}),
                    ("process_stop", {"process_id": "missing"}),
                ]:
                    with self.subTest(name=name, arguments=arguments):
                        with self.assertRaises(ToolRefusedError):
                            await handler(name, arguments)
                with self.assertRaises(ToolIOError):
                    await handler("process_start", {"command": "pwd", "cwd": "missing"})
                await handler("process_start", {"command": "cat"})
                with self.assertRaises(ToolFailedError):
                    await handler("process_start", {"command": "cat"})
            with self.assertRaises(ToolRefusedError):
                await handler("process_start", {"command": "pwd"})

    async def test_agent_uses_all_five_tools_across_turns_without_implicit_eof(self) -> None:
        async with ProcessSession(self.root) as session:

            class ObservedProvider(MockAIProvider):
                async def generate(self, context: AIContext) -> AIResponse:
                    # Synchronize the scripted provider with real pipe capture,
                    # without changing the non-waiting process_read contract.
                    if len(self.calls) == 3:
                        await wait_job(session, "test-room", "agent", "p1", stdout_bytes=8)
                    elif len(self.calls) == 6:
                        await wait_job(
                            session,
                            "test-room",
                            "agent",
                            "p1",
                            state="finished",
                            output_complete=True,
                        )
                    return await super().generate(context)

            script = [
                ("process_start", {"command": "cat"}),
                None,
                ("process_write", {"process_id": "p1", "text": "Welcome\n"}),
                ("process_read", {"process_id": "p1"}),
                None,
                ("process_write", {"process_id": "p1", "close_stdin": True}),
                ("process_status", {"process_id": "p1"}),
                None,
                ("process_stop", {"process_id": "p1", "forget": True}),
                None,
            ]
            provider = ObservedProvider(
                ai_responses=[
                    AIResponse(
                        content="",
                        finish_reason="tool_calls",
                        tool_calls=[
                            AIToolCall(id=f"call-{index}", name=item[0], arguments=item[1])
                        ],
                    )
                    if item is not None
                    else AIResponse(content="Operation completed.")
                    for index, item in enumerate(script)
                ],
                streaming=False,
            )
            channel = AIChannel(
                "agent", provider=provider, tools=[adapter(session) for adapter in ADAPTERS]
            )
            binding = ChannelBinding(
                room_id="test-room",
                channel_id="agent",
                channel_type=ChannelType.AI,
                category=ChannelCategory.INTELLIGENCE,
            )
            context = RoomContext(room=Room(id="test-room"), bindings=[binding])
            results = []
            try:
                for request in ["Start cat", "Send and read", "Send EOF", "Forget the job"]:
                    response = await channel.on_event(
                        RoomEvent(
                            room_id="test-room",
                            source=EventSource(channel_id="user", channel_type=ChannelType.SMS),
                            content=TextContent(body=request),
                        ),
                        binding,
                        context,
                    )
                    if response.response_stream is not None:
                        async for _ in response.response_stream:
                            pass
                    parts = [
                        part
                        for message in provider.calls[-1].messages
                        if isinstance(message.content, list)
                        for part in message.content
                        if isinstance(part, AIToolResultPart)
                    ]
                    self.assertFalse(any(part.is_error for part in parts))
                    results.extend(json.loads(part.result)["data"] for part in parts)
            finally:
                await channel.close()
            self.assertEqual(len(provider.calls), 10)
            self.assertEqual(len(results), 6)
            start, write, read, eof, status, stop = results
            statuses = status["processes"]
            self.assertEqual(start["process_id"], "p1")
            self.assertTrue(write["complete"])
            self.assertEqual(write["bytes_written"], 8)
            self.assertFalse(write["stdin_closed"])
            self.assertEqual(read["stdout"]["bytes"], list(b"Welcome\n"))
            self.assertEqual(read["status"]["state"], "running")
            self.assertFalse(read["status"]["stdin_closed"])
            self.assertTrue(eof["stdin_closed"])
            self.assertEqual(statuses[0]["exit_code"], 0)
            self.assertIsNone(statuses[0]["signal"])
            self.assertEqual(statuses[0]["end"], "exited")
            self.assertTrue(stop["forgotten"])
            with in_call("test-room", "agent"):
                status = await ProcessStatus(session).handler("process_status", {})
            self.assertEqual(json.loads(status)["data"], {"processes": []})

    async def test_concurrent_adapters_share_session_but_separate_sessions_are_isolated(
        self,
    ) -> None:
        with in_call("room", "agent"):
            async with ProcessSession(self.root) as first, ProcessSession(self.root) as second:
                results = await asyncio.gather(
                    *[
                        ProcessStart(first).handler("process_start", {"command": "cat"})
                        for _ in range(3)
                    ]
                )
                self.assertEqual(
                    {json.loads(result)["data"]["process_id"] for result in results},
                    {"p1", "p2", "p3"},
                )
                status = json.loads(await ProcessStatus(first).handler("process_status", {}))
                self.assertEqual(len(status["data"]["processes"]), 3)
                with self.assertRaises(ToolRefusedError):
                    await ProcessRead(second).handler("process_read", {"process_id": "p1"})
                status = json.loads(await ProcessStatus(second).handler("process_status", {}))
                self.assertEqual(status["data"], {"processes": []})

    async def test_rooms_and_agents_sharing_a_session_only_reach_their_own_jobs(self) -> None:
        async with ProcessSession(self.root) as session:
            _, handler = extract_tools([adapter(session) for adapter in ADAPTERS])
            with in_call("room-a", "agent"):
                started = json.loads(await handler("process_start", {"command": "cat"}))
                job = started["data"]["process_id"]
            for room, agent in [("room-b", "agent"), ("room-a", "other-agent")]:
                with self.subTest(room=room, agent=agent), in_call(room, agent):
                    listed = json.loads(await handler("process_status", {}))
                    self.assertEqual(listed["data"], {"processes": []})
                    for name, arguments in [
                        ("process_status", {"process_id": job}),
                        ("process_read", {"process_id": job}),
                        ("process_write", {"process_id": job, "text": "x"}),
                        ("process_stop", {"process_id": job}),
                    ]:
                        with self.assertRaises(ToolRefusedError):
                            await handler(name, arguments)
                    own = json.loads(await handler("process_start", {"command": "true"}))
                    self.assertEqual(own["data"]["process_id"], "p2" if room == "room-b" else "p3")
            with in_call("room-a", "agent"):
                listed = json.loads(await handler("process_status", {}))
                self.assertEqual([s["process_id"] for s in listed["data"]["processes"]], [job])
                written = json.loads(
                    await handler("process_write", {"process_id": job, "text": "a\n"})
                )
                self.assertTrue(written["data"]["complete"])
                stopped = json.loads(
                    await handler("process_stop", {"process_id": job, "forget": True})
                )
                self.assertTrue(stopped["data"]["forgotten"])
            for name in ("process_start", "process_status"):
                with self.assertRaises(ToolRefusedError):
                    await handler(name, {"command": "true"} if name == "process_start" else {})
            self.assertEqual((await session.status())["data"]["processes"], [])

    def test_owner_key_is_the_one_the_rust_adapter_returns(self) -> None:
        # The Rust adapter's tests check the same cases against its roomkit_owner.
        cases = json.loads(OWNER_FIXTURE.read_text(encoding="utf-8"))["cases"]
        for case in cases:
            with self.subTest(case=case):
                self.assertEqual(roomkit_owner(case["room_id"], case["channel_id"]), case["owner"])
        for case in cases[:2]:
            with self.subTest(case=case), in_call(case["room_id"], case["channel_id"]):
                self.assertEqual(_roomkit_owner("process"), case["owner"])
        with self.assertRaises(TypeError):
            roomkit_owner("room", 1)

    async def test_ids_that_are_not_unicode_text_are_refused(self) -> None:
        # Python strings can hold lone surrogates; RoomKit Rust ids cannot.
        for room, agent in [("room\ud800", "agent"), ("room", "agent\udfff")]:
            with self.subTest(room=room, agent=agent), self.assertRaises(InvalidInputError):
                roomkit_owner(room, agent)
        async with OutputStore(self.root) as outputs, ProcessSession(self.root) as session:
            tools = [
                ProcessStart(session),
                OutputRead(outputs),
                Shell(await ShellTool.create(self.root, outputs=outputs)),
            ]
            calls = [
                ("process_start", {"command": "true"}),
                ("output_read", {"archive_id": "o1"}),
                ("shell", {"command": "true"}),
            ]
            for tool, (name, arguments) in zip(tools, calls, strict=True):
                with (
                    self.subTest(name=name),
                    in_call("room\ud800", "agent"),
                    self.assertRaises(ToolRefusedError),
                ):
                    await tool.handler(name, arguments)

    async def test_host_reaches_a_room_s_jobs_and_archives_through_its_owner(self) -> None:
        async with (
            OutputStore(self.root, max_archives_per_owner=4) as outputs,
            ProcessSession(self.root, outputs=outputs, max_processes_per_owner=2) as session,
        ):
            _, handler = extract_tools([adapter(session) for adapter in ADAPTERS])
            with in_call("room-a", "agent"):
                jobs = [
                    json.loads(await handler("process_start", {"command": command}))["data"]
                    for command in ("cat", "sleep 60")
                ]
                with self.assertRaises(ToolFailedError):
                    await handler("process_start", {"command": "true"})
            owner = roomkit_owner("room-a", "agent")
            cat, sleeper = (job["process_id"] for job in jobs)
            self.assertEqual((await session.status())["data"]["processes"], [])
            self.assertEqual(await outputs.list(), [])
            listed = (await session.status(owner=owner))["data"]["processes"]
            self.assertEqual([status["process_id"] for status in listed], [cat, sleeper])
            await session.write(cat, "hello\n", close_stdin=True, owner=owner)
            await wait_job(session, "room-a", "agent", cat, state="finished", output_complete=True)
            page = (await session.read(cat, owner=owner))["data"]
            self.assertEqual(page["stdout"]["text"], "hello\n")
            stopped = (await session.stop(sleeper, forget=True, owner=owner))["data"]
            self.assertTrue(stopped["forgotten"])
            self.assertTrue(await session.close_owner(owner))
            self.assertEqual((await session.status(owner=owner))["data"]["processes"], [])
            archive = jobs[0]["stdout_archive"]["archive_id"]
            copy = (await outputs.read(archive, owner=owner))["data"]
            self.assertEqual(copy["text"], "hello\n")
            archives = await outputs.list(owner=owner)
            self.assertEqual(len(archives), 4)
            for entry in archives:
                await outputs.forget(entry["archive_id"], owner=owner)
            # Both quotas are free again for that room and agent.
            with in_call("room-a", "agent"):
                again = json.loads(await handler("process_start", {"command": "true"}))["data"]
            self.assertIsNone(again["stdout_archive"]["error"])

    async def test_cancelled_handler_write_preserves_job_and_partial_stdin(self) -> None:
        async with ProcessSession(
            self.root, max_write_bytes=1_000_000, write_timeout_ms=5000
        ) as session:
            with in_call("room", "agent"):
                started = await ProcessStart(session).handler(
                    "process_start", {"command": "sleep 60"}
                )
                process_id = json.loads(started)["data"]["process_id"]
                task = asyncio.create_task(
                    ProcessWrite(session).handler(
                        "process_write", {"process_id": process_id, "text": "x" * 1_000_000}
                    )
                )
            await asyncio.sleep(0.02)
            task.cancel()
            with self.assertRaises(asyncio.CancelledError):
                await task
            status = await wait_job(session, "room", "agent", process_id)
            self.assertEqual(status["state"], "running")
            self.assertFalse(status["stdin_closed"])


class RoomKitTwoRoomsTests(unittest.IsolatedAsyncioTestCase):
    """One agent attached to two rooms, driven through RoomKit's own tool loop."""

    async def test_one_agent_in_two_rooms_keeps_jobs_and_archives_apart(self) -> None:
        # p1 captures into o1/o2; the shell call in room-a writes o3/o4.
        turns = [
            ("room-a", [("process_start", {"command": "cat"}), ("shell", {"command": "printf a"})]),
            (
                "room-b",
                [
                    ("process_status", {}),
                    ("process_read", {"process_id": "p1"}),
                    ("process_write", {"process_id": "p1", "text": "x"}),
                    ("process_stop", {"process_id": "p1"}),
                    ("output_read", {"archive_id": "o3"}),
                ],
            ),
            ("room-a", [("process_status", {}), ("output_read", {"archive_id": "o3"})]),
        ]
        with tempfile.TemporaryDirectory() as directory:
            events = await self._run(Path(directory), turns)
        # Calls of one round run concurrently; key their outcomes by tool name.
        room_a, room_b = (
            {e.name: (e.is_error, e.result) for e in events if e.room_id == room}
            for room in ("room-a", "room-b")
        )
        self.assertEqual(len(room_b), 5, room_b)
        status_error, status = room_b.pop("process_status")
        self.assertFalse(status_error)
        self.assertEqual(json.loads(status)["data"], {"processes": []})
        self.assertTrue(all(is_error for is_error, _ in room_b.values()), room_b)
        self.assertFalse(any(is_error for is_error, _ in room_a.values()), room_a)
        listed = json.loads(room_a["process_status"][1])["data"]["processes"]
        self.assertEqual([(s["process_id"], s["state"]) for s in listed], [("p1", "running")])
        self.assertEqual(json.loads(room_a["output_read"][1])["data"]["text"], "a")

    async def _run(self, root: Path, turns) -> list[ToolCallEvent]:
        responses = []
        for index, (_, calls) in enumerate(turns):
            tool_calls = [
                AIToolCall(id=f"call-{index}-{n}", name=name, arguments=arguments)
                for n, (name, arguments) in enumerate(calls)
            ]
            responses += [
                AIResponse(content="", finish_reason="tool_calls", tool_calls=tool_calls),
                AIResponse(content="Done."),
            ]
        events: list[ToolCallEvent] = []
        async with (
            OutputStore(root) as outputs,
            ProcessSession(root, outputs=outputs) as session,
            RoomKit() as kit,
        ):
            tools = [adapter(session) for adapter in ADAPTERS]
            tools += [Shell(await ShellTool.create(root, outputs=outputs)), OutputRead(outputs)]
            provider = MockAIProvider(ai_responses=responses, streaming=False)
            kit.register_channel(AIChannel("agent", provider=provider, tools=tools))
            kit.register_channel(WebSocketChannel("user"))

            @kit.hook(HookTrigger.ON_TOOL_CALL, execution=HookExecution.ASYNC)
            async def observe(event: ToolCallEvent, _context: RoomContext) -> HookResult:
                if event.result is not None:
                    events.append(event)
                return HookResult.allow()

            for room in ("room-a", "room-b"):
                await kit.create_room(room_id=room)
                await kit.attach_channel(room, "agent", category=ChannelCategory.INTELLIGENCE)
                await kit.attach_channel(room, "user")
            for room, _ in turns:
                message = InboundMessage(
                    channel_id="user", sender_id="developer", content=TextContent(body="Go")
                )
                await kit.process_inbound(message, room_id=room)
        return events


if __name__ == "__main__":
    unittest.main()
