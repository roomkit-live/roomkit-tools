"""ask_user served by RoomKit's human input, through a real kit."""

import asyncio
import unittest

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
from roomkit.providers.ai import AIResponse, AIToolCall, AIToolResultPart, MockAIProvider
from roomkit_tools import AskUserTool, InvalidInputError, ask_user_handler

QUESTIONS = {
    "questions": [
        {
            "header": "Database",
            "question": "Which database?",
            "options": [{"label": "SQLite"}, {"label": "Postgres"}],
        }
    ]
}


class AskUserThroughRoomKit(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.kit = RoomKit()
        await self.kit.create_room(room_id="room")
        user = WebSocketChannel("user")

        async def receive(connection, event):
            pass

        user.register_connection("console", receive, room_id="room")
        self.kit.register_channel(user)
        await self.kit.attach_channel("room", "user")
        self.asked = []

        @self.kit.hook(HookTrigger.ON_USER_INPUT_REQUIRED, execution=HookExecution.SYNC)
        async def show(event, context):
            # The host's view: read the questions, then answer, or refuse
            # with what the model should fix.
            try:
                questions = AskUserTool.read(event.arguments)["questions"]
            except InvalidInputError as error:
                await self.kit.reject_input(event.pending_id, str(error))
                return HookResult.allow()
            self.asked.append(questions)
            answer = f"{questions[0]['header']}: {questions[0]['options'][0]['label']}"
            await self.kit.answer_input(event.pending_id, answer)
            return HookResult.allow()

    async def asyncTearDown(self):
        await self.kit.close()

    async def turn(self, arguments):
        provider = MockAIProvider(
            ai_responses=[
                AIResponse(
                    content="",
                    finish_reason="tool_calls",
                    tool_calls=[AIToolCall(id="call-ask", name="ask_user", arguments=arguments)],
                ),
                AIResponse(content="Noted."),
            ],
            streaming=False,
        )
        channel = AIChannel(
            "assistant", provider=provider, human_input_handler=ask_user_handler(timeout=5)
        )
        self.kit.register_channel(channel)
        await self.kit.attach_channel("room", "assistant", category=ChannelCategory.INTELLIGENCE)
        async with asyncio.timeout(20):
            await self.kit.process_inbound(
                InboundMessage(
                    channel_id="user", sender_id="person", content=TextContent(body="Set it up")
                ),
                room_id="room",
            )
        return provider

    @staticmethod
    def read_by_model(provider):
        return [
            part
            for message in provider.calls[-1].messages
            if isinstance(message.content, list)
            for part in message.content
            if isinstance(part, AIToolResultPart)
        ]

    async def test_the_model_is_offered_the_tool_and_reads_the_answer(self):
        provider = await self.turn(QUESTIONS)
        offered = {tool.name: tool for tool in provider.calls[0].tools}
        self.assertEqual(offered["ask_user"].parameters, AskUserTool.definition()["input_schema"])
        self.assertEqual(self.asked[0][0]["question"], "Which database?")
        [result] = self.read_by_model(provider)
        self.assertEqual(result.result, "Database: SQLite")
        self.assertFalse(result.is_error)

    async def test_the_model_reads_what_to_fix_in_its_call(self):
        provider = await self.turn({"questions": [{"question": "  "}]})
        self.assertEqual(self.asked, [])
        [result] = self.read_by_model(provider)
        self.assertTrue(result.is_error)
        self.assertIn("Question 1 has no text.", result.result)
