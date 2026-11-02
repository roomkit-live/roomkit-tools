"""Installed native HTTP transport: parity, policy, cancellation and schemas."""

import asyncio
import inspect
import json
import os
import unittest
from contextlib import nullcontext
from pathlib import Path
from unittest.mock import patch

from roomkit_tools import AccessRefusedError, InvalidInputError, WebSearchTool, _native
from roomkit_tools.search import ExaSearch, OllamaSearch, OpenAISearch
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
                    backend_type = {
                        "exa": ExaSearch,
                        "ollama": OllamaSearch,
                        "openai": OpenAISearch,
                    }[case["backend"]]
                    options = (
                        {"model": "fixture-model", **case.get("backend_options", {})}
                        if case["backend"] == "openai"
                        else {}
                    )
                    backend = backend_type(
                        api_key="fixture-key",
                        **options,
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
            exa = {case["path"] for case in FIXTURE["cases"] if case["backend"] == "exa"}
            sent = [request for request in server.seen if request["path"] in exa]
            self.assertTrue(sent)
            for request in sent:
                self.assertEqual(request["headers"].get("x-api-key"), "fixture-key", request)

    async def test_openai_wire_contract_and_visible_citations(self):
        async with SearchServer() as server:
            backend = OpenAISearch(
                api_key="fixture-key",
                model="fixture-model",
                endpoint=server.origin + "/openai/answer",
                private_origins=[server.origin],
                max_tool_calls=2,
                max_output_tokens=300,
            )
            output = await WebSearchTool(backend=backend).search("question", limit=3)
            body = server.seen[0]["body"]
            self.assertEqual(body["model"], "fixture-model")
            self.assertEqual(body["max_tool_calls"], 2)
            self.assertEqual(body["max_output_tokens"], 300)
            self.assertFalse(body["store"])
            self.assertFalse(body["stream"])
            self.assertNotIn("previous_response_id", body)
            self.assertNotIn("fixture-key", json.dumps(body))
            self.assertEqual(backend.capabilities["result_kinds"], ["answer", "not_performed"])
            self.assertIn("Café ☕ [source 1](<https://example.com/guide>)", output["text"])
            self.assertEqual(output["data"]["payload"]["citations"][0]["end"], 9)

    async def test_openai_configuration_and_cancel_during_response(self):
        for options in [
            {"model": ""},
            {"model": " x"},
            {"max_tool_calls": 0},
            {"max_tool_calls": 17},
            {"max_output_tokens": 0},
        ]:
            with self.subTest(options=options), self.assertRaises(InvalidInputError):
                OpenAISearch(api_key="fixture-key", **{"model": "fixture-model", **options})
        async with SearchServer() as server:
            tool = WebSearchTool(
                backend=OpenAISearch(
                    api_key="fixture-key",
                    model="fixture-model",
                    endpoint=server.origin + "/openai/deadline",
                    private_origins=[server.origin],
                )
            )
            task = asyncio.create_task(tool.search("q"))
            await asyncio.wait_for(server.received.wait(), 5)
            task.cancel()
            with self.assertRaises(asyncio.CancelledError):
                await asyncio.wait_for(task, 2)
            self.assertEqual((await tool.search("next"))["data"]["provider"], "openai")

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
        # The key is checked before the transport, as the other backends do.
        with self.assertRaises(InvalidInputError):
            ExaSearch(api_key=" key", endpoint="http://example.com/search")
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


class CapabilitiesTests(unittest.TestCase):
    def test_the_tool_reports_its_backends_capabilities(self):
        """WebSearchTool.capabilities(), as Rust's WebSearchTool::capabilities()."""
        for backend in (
            ExaSearch(allowed_origins=[]),
            OpenAISearch(api_key="fixture-key", model="fixture-model"),
        ):
            with self.subTest(backend=type(backend).__name__):
                tool = WebSearchTool(backend=backend)
                self.assertEqual(tool.capabilities(), backend.capabilities)


class SearchLoopTests(unittest.TestCase):
    def test_backend_and_tool_reuse_across_event_loops(self):
        tool = WebSearchTool(backend=ExaSearch(allowed_origins=[]))

        async def refused():
            with self.assertRaises(AccessRefusedError):
                await tool.search("q")

        asyncio.run(refused())
        asyncio.run(refused())


class NamedParametersTests(unittest.TestCase):
    """Each limit is a named keyword, the fields of Rust's SearchLimits: a misspelled one
    fails when it is passed, as any Python keyword."""

    def test_web_search_tool_create_names_every_limit(self):
        self.assertEqual(
            list(inspect.signature(WebSearchTool.create).parameters),
            [
                "backend",
                "max_query_bytes",
                "max_results",
                "max_content_bytes",
                "max_output_bytes",
                "default_timeout_ms",
                "max_timeout_ms",
            ],
        )

    def test_create_takes_the_constructors_parameters(self):
        create = inspect.signature(WebSearchTool.create).parameters
        init = dict(inspect.signature(WebSearchTool.__init__).parameters)
        init.pop("self")
        self.assertEqual(
            [(name, p.kind, p.default) for name, p in create.items()],
            [(name, p.kind, p.default) for name, p in init.items()],
        )

    def test_a_limit_given_to_create_reaches_the_tool(self):
        async def search():
            backend = ExaSearch(allowed_origins=[])
            tool = await WebSearchTool.create(backend=backend, max_query_bytes=4)
            with self.assertRaises(InvalidInputError):
                await tool.search("a query longer than four bytes")
            with self.assertRaises(AccessRefusedError):
                await tool.search("abcd")

        asyncio.run(search())
