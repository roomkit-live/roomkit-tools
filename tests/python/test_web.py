"""Installed native HTTP transport: parity, policy, cancellation and schemas."""

import asyncio
import inspect
import json
import os
import unittest
from contextlib import nullcontext
from pathlib import Path
from unittest.mock import patch

from roomkit_tools import (
    AccessRefusedError,
    InvalidInputError,
    LimitExceededError,
    WebFetchTool,
    _native,
)
from test_contracts import ERRORS, pointer
from web_helpers import FIXTURE, WebServer


def replace(value, origin, replacement):
    return json.loads(json.dumps(value).replace(origin, replacement))


class WebFetchTests(unittest.IsolatedAsyncioTestCase):
    async def test_all_shared_http_cases_and_complete_rust_envelopes(self):
        reference_path = os.getenv("ROOMKIT_WEB_RESULTS")
        reference = (
            json.loads(await asyncio.to_thread(Path(reference_path).read_text))
            if reference_path
            else None
        )
        if reference:
            self.assertEqual(reference["fixture"], FIXTURE, "stale Rust fixtures")
        async with WebServer() as server:
            for case in FIXTURE["cases"]:
                with self.subTest(case=case["name"]):
                    tool = WebFetchTool(
                        private_origins=[server.origin] if case.get("private", True) else [],
                        **case.get("limits", {}),
                    )
                    token = _native._Cancellation()
                    token.cancel()
                    scope = (
                        patch.object(_native, "_Cancellation", return_value=token)
                        if case.get("cancelled")
                        else nullcontext()
                    )
                    try:
                        with scope:
                            result = await tool.call(
                                replace(case["input"], "$ORIGIN", server.origin)
                            )
                    except tuple(ERRORS) as error:
                        result = {"error": ERRORS[type(error)]}
                    result = replace(result, server.origin, "$ORIGIN")
                    for key, expected in case["expect"].items():
                        self.assertEqual(pointer(result, key), expected)
                    if reference:
                        self.assertEqual(
                            json.dumps(result, sort_keys=True),
                            json.dumps(reference["reports"][case["name"]], sort_keys=True),
                        )

    async def test_cancel_in_flight_releases_call_and_reuse_still_works(self):
        async with WebServer() as server:
            tool = WebFetchTool(private_origins=[server.origin])
            task = asyncio.create_task(tool.fetch(server.origin + "/slow"))
            await asyncio.wait_for(server.received.wait(), 5)
            task.cancel()
            with self.assertRaises(asyncio.CancelledError):
                await asyncio.wait_for(task, 2)
            result = await tool.fetch(server.origin + "/json")
            self.assertEqual(result["data"]["content"], '{"ready":true}')

    async def test_policy_stops_requests_and_cannot_be_changed_by_model(self):
        async with WebServer() as server:
            tool = WebFetchTool(allowed_origins=[], private_origins=[server.origin])
            with self.assertRaises(AccessRefusedError):
                await tool.fetch(server.origin + "/doc")
            with self.assertRaises(InvalidInputError):
                await tool.call({"url": server.origin + "/doc", "private_origins": [server.origin]})
            self.assertEqual(server.seen, [])

    async def test_local_hostname_dns_is_checked_before_connecting(self):
        async with WebServer() as server:
            with self.assertRaises(AccessRefusedError):
                await WebFetchTool().fetch(server.origin.replace("127.0.0.1", "localhost") + "/doc")
            self.assertEqual(server.seen, [])

    async def test_configuration_and_actual_schema(self):
        definition = WebFetchTool.definition()
        self.assertEqual(definition, json.loads(_native._WebFetchTool.definition_json()))
        self.assertEqual(definition["name"], "web_fetch")
        self.assertEqual(
            set(definition["input_schema"]["properties"]), {"url", "format", "timeout_ms"}
        )
        self.assertFalse(definition["input_schema"]["additionalProperties"])
        for config in (
            {"max_output_bytes": 0},
            {"max_redirects": 33},
            {"private_origins": ["http://localhost/path"]},
        ):
            with self.subTest(config=config), self.assertRaises(InvalidInputError):
                WebFetchTool(**config)
        self.assertIsInstance(await WebFetchTool.create(), WebFetchTool)


class WebFetchLoopTests(unittest.TestCase):
    def test_tool_can_be_reused_across_event_loops(self):
        tool = WebFetchTool(allowed_origins=[])

        async def refused():
            with self.assertRaises(AccessRefusedError):
                await tool.fetch("https://example.com")

        asyncio.run(refused())
        asyncio.run(refused())


class NamedParametersTests(unittest.TestCase):
    """Each limit is a named keyword, the fields of Rust's WebFetchPolicy and
    WebFetchLimits: a misspelled one fails when it is passed, as any keyword."""

    def test_web_fetch_tool_create_names_every_limit(self):
        self.assertEqual(
            list(inspect.signature(WebFetchTool.create).parameters),
            [
                "allowed_origins",
                "private_origins",
                "max_url_bytes",
                "max_response_bytes",
                "max_converted_bytes",
                "max_output_bytes",
                "max_redirects",
                "default_timeout_ms",
                "max_timeout_ms",
            ],
        )

    def test_create_takes_the_constructors_parameters(self):
        create = inspect.signature(WebFetchTool.create).parameters
        init = dict(inspect.signature(WebFetchTool.__init__).parameters)
        init.pop("self")
        self.assertEqual(
            [(name, p.kind, p.default) for name, p in create.items()],
            [(name, p.kind, p.default) for name, p in init.items()],
        )

    def test_a_limit_given_to_create_reaches_the_tool(self):
        async def fetch():
            tool = await WebFetchTool.create(allowed_origins=[], max_url_bytes=32)
            with self.assertRaises(LimitExceededError):
                await tool.fetch("https://example.com/" + "a" * 64)
            with self.assertRaises(AccessRefusedError):
                await tool.fetch("https://example.com/")

        asyncio.run(fetch())

    def test_a_zero_timeout_is_refused_and_one_accepted(self):
        async def fetch():
            tool = await WebFetchTool.create(allowed_origins=[])
            with self.assertRaises(InvalidInputError):
                await tool.fetch("https://example.com/", timeout_ms=0)
            # 1 is a valid deadline: the call reaches the origin policy.
            with self.assertRaises(AccessRefusedError):
                await tool.fetch("https://example.com/", timeout_ms=1)

        asyncio.run(fetch())

    def test_a_timeout_above_the_maximum_is_refused_with_it(self):
        async def fetch():
            tool = await WebFetchTool.create(
                allowed_origins=[], default_timeout_ms=1000, max_timeout_ms=1000
            )
            with self.assertRaisesRegex(
                InvalidInputError, "timeout_ms exceeds the host maximum of 1000 ms"
            ):
                await tool.fetch("https://example.com/", timeout_ms=1001)
            # The maximum is a valid deadline: the call reaches the origin policy.
            with self.assertRaises(AccessRefusedError):
                await tool.fetch("https://example.com/", timeout_ms=1000)

        asyncio.run(fetch())
