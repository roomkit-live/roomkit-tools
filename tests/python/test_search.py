"""Installed native HTTP transport: parity, policy, cancellation and schemas."""

import asyncio
import json
import os
import unittest
from contextlib import nullcontext
from pathlib import Path
from unittest.mock import patch

from roomkit_tools import AccessRefusedError, InvalidInputError, WebSearchTool, _native
from roomkit_tools.search import ExaSearch, OllamaSearch
from search_helpers import FIXTURE, SearchServer
from test_contracts import ERRORS, pointer


def replace(value, origin, replacement):
    return json.loads(json.dumps(value).replace(origin, replacement))


class WebSearchTests(unittest.IsolatedAsyncioTestCase):
    async def test_all_shared_http_cases_and_complete_rust_envelopes(self):
        reference_path = os.getenv("ROOMKIT_SEARCH_RESULTS")
        reference = (
            json.loads(await asyncio.to_thread(Path(reference_path).read_text))
            if reference_path
            else None
        )
        if reference:
            self.assertEqual(reference["fixture"], FIXTURE, "stale Rust fixtures")
        async with SearchServer() as server:
            for case in FIXTURE["cases"]:
                with self.subTest(case=case["name"]):
                    backend_type = ExaSearch if case["backend"] == "exa" else OllamaSearch
                    backend = backend_type(
                        api_key="fixture-key",
                        endpoint=server.origin + case["path"],
                        private_origins=[server.origin],
                        allowed_origins=[] if case.get("deny") else None,
                        max_response_bytes=case.get("max_response_bytes", 512 * 1024),
                    )
                    tool = WebSearchTool(backend=backend, **case.get("limits", {}))
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

    async def test_cancellation_during_body_and_subsequent_reuse(self):
        async with SearchServer() as server:
            tool = WebSearchTool(
                backend=OllamaSearch(
                    api_key="test",
                    endpoint=server.origin + "/slow",
                    private_origins=[server.origin],
                )
            )
            task = asyncio.create_task(tool.search("query"))
            await asyncio.wait_for(server.received.wait(), 5)
            task.cancel()
            with self.assertRaises(asyncio.CancelledError):
                await asyncio.wait_for(task, 2)
            self.assertEqual((await tool.search("next"))["data"]["provider"], "ollama")

    async def test_request_shape_headers_and_redirect_not_followed(self):
        async with SearchServer() as server:
            for cls, path in [(OllamaSearch, "/ollama"), (ExaSearch, "/exa")]:
                tool = WebSearchTool(
                    backend=cls(
                        api_key="fixture-key",
                        endpoint=server.origin + path,
                        private_origins=[server.origin],
                    )
                )
                await tool.search("shipping policy", limit=2)
            self.assertEqual(server.seen[0]["body"], {"query": "shipping policy", "max_results": 2})
            self.assertEqual(server.seen[0]["headers"]["authorization"], "Bearer fixture-key")
            self.assertEqual(
                server.seen[1]["body"]["params"],
                {
                    "name": "web_search_exa",
                    "arguments": {"query": "shipping policy", "numResults": 2},
                },
            )
            self.assertEqual(server.seen[1]["headers"]["x-api-key"], "fixture-key")
            before = len(server.seen)
            with self.assertRaises(AccessRefusedError):
                await WebSearchTool(
                    backend=ExaSearch(
                        endpoint=server.origin + "/redirect", private_origins=[server.origin]
                    )
                ).search("q")
            self.assertEqual(len(server.seen), before + 1)

    async def test_model_cannot_configure_backend_or_origin(self):
        tool = WebSearchTool(backend=ExaSearch(allowed_origins=[]))
        with self.assertRaises(InvalidInputError):
            await tool.call({"query": "q", "api_key": "should-not-be-allowed"})
        with self.assertRaises(AccessRefusedError):
            await tool.search("q")
        definition = tool.definition()
        self.assertEqual(
            set(definition["input_schema"]["properties"]), {"query", "limit", "timeout_ms"}
        )
        self.assertFalse(definition["input_schema"]["additionalProperties"])

    async def test_credentials_configuration_and_error_messages(self):
        for key in ["", " key", "x\r\ny"]:
            with self.assertRaises(InvalidInputError):
                OllamaSearch(api_key=key)
        with self.assertRaises(AccessRefusedError):
            ExaSearch(endpoint="http://example.com/search")
        with self.assertRaises(InvalidInputError):
            ExaSearch(endpoint="https://example.com/search?key=secret")
        async with SearchServer() as server:
            with self.assertRaises(AccessRefusedError) as ctx:
                await WebSearchTool(
                    backend=OllamaSearch(
                        api_key="secret",
                        endpoint=server.origin + "/auth",
                        private_origins=[server.origin],
                    )
                ).search("q")
            self.assertNotIn("private diagnostic", str(ctx.exception))
            self.assertNotIn("secret", str(ctx.exception))


class SearchLoopTests(unittest.TestCase):
    def test_backend_and_tool_reuse_across_event_loops(self):
        tool = WebSearchTool(backend=ExaSearch(allowed_origins=[]))

        async def refused():
            with self.assertRaises(AccessRefusedError):
                await tool.search("q")

        asyncio.run(refused())
        asyncio.run(refused())
