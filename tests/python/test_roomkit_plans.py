"""Plan ownership comes from RoomKit context, never tool arguments."""

import json
import unittest

from roomkit.core.exceptions import ToolRefusedError, UnservedToolCallError
from roomkit.tools.context import ToolCallContext, tool_turn_context
from roomkit_tools import MemoryPlanBackend, PlanCreate, PlanList, PlanRead, PlanUpdate


class RoomKitPlansTests(unittest.IsolatedAsyncioTestCase):
    async def test_trusted_room_and_channel_scope_and_structured_results(self):
        backend = MemoryPlanBackend()
        creator = PlanCreate(backend)
        arguments = {"title": "Inspect", "objective": "Check the threshold", "steps": []}
        with self.assertRaises(ToolRefusedError):
            await creator.handler("plan_create", arguments)
        call = ToolCallContext(room_id="conversation", channel_id="assistant")
        with tool_turn_context(room_id="conversation", actor_id="user", call=call):
            result = json.loads(await creator.handler("plan_create", arguments))
            self.assertEqual(call.structured_content, result)
            plan = result["data"]
            with self.assertRaises(ToolRefusedError):
                await creator.handler("plan_create", {**arguments, "caller_id": "other"})
            with self.assertRaises(UnservedToolCallError):
                await creator.handler("plan_delete", {})
            with self.assertRaises(ToolRefusedError):
                await PlanUpdate(backend).handler(
                    "plan_update",
                    {"plan_id": plan["plan_id"], "expected_revision": 9, "title": "Changed"},
                )
        for room, channel in (("other", "assistant"), ("conversation", "other")):
            with tool_turn_context(
                room_id=room,
                actor_id="user",
                call=ToolCallContext(room_id=room, channel_id=channel),
            ):
                result = json.loads(await PlanList(backend).handler("plan_list", {}))
                self.assertEqual(result["data"]["plans"], [])
                with self.assertRaises(ToolRefusedError):
                    await PlanRead(backend).handler("plan_read", {"plan_id": plan["plan_id"]})
