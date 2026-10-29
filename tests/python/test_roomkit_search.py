"""Real RoomKit loop and independently selected fetch capability."""

import importlib.util
import io
import json
import unittest
from contextlib import asynccontextmanager, redirect_stderr, redirect_stdout
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import AsyncMock, patch

from roomkit.core.exceptions import ToolFailedError, ToolRefusedError, UnservedToolCallError
from roomkit.providers.ai import AIResponse, AIToolCall, MockAIProvider
from roomkit.tools.compose import extract_tools
from roomkit_tools import WebSearch
from roomkit_tools.search import ExaSearch
from search_helpers import SearchServer

EXAMPLE = Path(__file__).resolve().parents[2] / "examples/roomkit_search.py"


def load_example():
    spec = importlib.util.spec_from_file_location("search_example", EXAMPLE)
    example = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(example)
    return example


def event(name, data, *, is_error=False):
    return SimpleNamespace(name=name, result=json.dumps({"data": data}), is_error=is_error)


class RoomKitSearchTests(unittest.IsolatedAsyncioTestCase):
    async def test_agent_searches_then_fetches_returned_source(self):
        spec = importlib.util.spec_from_file_location("web_example", EXAMPLE)
        example = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(example)
        with redirect_stdout(io.StringIO()) as output:
            events = await example.run()
        self.assertEqual([e.name for e in events], ["web_search", "web_fetch"])
        self.assertIn("Verified:", output.getvalue())

    async def test_agent_consumes_cited_answer_from_native_openai_backend(self):
        spec = importlib.util.spec_from_file_location("search_answer_example", EXAMPLE)
        example = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(example)
        with redirect_stdout(io.StringIO()):
            events = await example.run(service="fixture-openai")
        output = json.loads(events[0].result)
        self.assertEqual(output["data"]["payload"]["kind"], "answer")
        self.assertEqual(output["data"]["usage"]["search_requests"], 1)
        self.assertIn("[source 1](<http://127.0.0.1:", output["text"])
        self.assertEqual(events[1].name, "web_fetch")
        self.assertTrue(json.loads(events[1].result)["data"]["url"].endswith("?utm_source=openai"))

    async def test_agent_corrects_removed_tracking_query_before_fetch_runs(self):
        example = load_example()
        documentation = example.documentation
        requests = []

        @asynccontextmanager
        async def capture_requests():
            async with documentation() as (origin, seen):
                yield origin, seen
                requests.extend(seen)

        def scripted_with_mistake(url):
            responses = [
                ("web_search", {"query": "shipping policy", "limit": 2}),
                ("web_fetch", {"url": url.split("?", 1)[0]}),
                ("web_fetch", {"url": url}),
            ]
            return MockAIProvider(
                streaming=False,
                ai_responses=[
                    AIResponse(
                        content="",
                        finish_reason="tool_calls",
                        tool_calls=[AIToolCall(id=str(i), name=name, arguments=args)],
                    )
                    for i, (name, args) in enumerate(responses)
                ]
                + [AIResponse(content="The fetched guide confirms the threshold.")],
            )

        with (
            patch.object(example, "documentation", capture_requests),
            patch.object(example, "scripted", scripted_with_mistake),
            redirect_stdout(io.StringIO()) as output,
        ):
            events = await example.run(service="fixture-openai")
        self.assertEqual([e.is_error for e in events], [False, True, False])
        self.assertIn("including query parameters", events[1].result)
        self.assertEqual(
            [target for kind, target in requests if kind == "fetch"],
            ["/guide?utm_source=openai"],
        )
        self.assertIn("Verified:", output.getvalue())

    async def test_cited_fragment_survives_fetch_normalization(self):
        example = load_example()
        answer, scripted = example.fixture_answer, example.scripted
        with (
            patch.object(example, "fixture_answer", lambda url: answer(url + "#shipping")),
            patch.object(example, "scripted", lambda url: scripted(url + "#shipping")),
            redirect_stdout(io.StringIO()),
        ):
            events = await example.run(service="fixture-openai")
        self.assertEqual([e.is_error for e in events], [False, False])
        fetched = json.loads(events[1].result)["data"]
        self.assertNotIn("#", fetched["url"])
        self.assertTrue(fetched["http_ok"])

    def test_verification_accepts_http_normalization_but_preserves_queries(self):
        example = load_example()
        source = "https://example.com/?utm_source=openai"
        search = event("web_search", {"payload": {"sources": [{"url": source}]}})
        for url, accepted in [
            ("https://EXAMPLE.com:443?utm_source=openai#section", True),
            ("https://example.com/", False),
            ("https://example.com/?utm_source=other", False),
            ("https://example.com/?page=2&utm_source=openai", False),
            ("https://other.example/?utm_source=openai", False),
            ("http://example.com/?utm_source=openai", False),
            ("https://example.com/other?utm_source=openai", False),
        ]:
            with self.subTest(url=url):
                fetch = event("web_fetch", {"url": url, "http_ok": True, "status": 200})
                if accepted:
                    example.verify_search_flow([search, fetch])
                else:
                    with self.assertRaisesRegex(example.SearchVerificationError, "returned_urls"):
                        example.verify_search_flow([search, fetch])

    def test_verification_requires_prior_search_and_successful_fetch(self):
        example = load_example()
        source = "https://example.com/"
        search = event("web_search", {"payload": {"results": [{"url": source}]}})
        fetch = event("web_fetch", {"url": source, "http_ok": True, "status": 200})
        cases = [
            [fetch],
            [fetch, search],
            [search],
            [search, event("web_fetch", {"url": source, "http_ok": False, "status": 404})],
            [event("web_search", {"payload": {"kind": "not_performed"}}), fetch],
            [event("web_search", "provider failure", is_error=True), fetch],
        ]
        for events in cases:
            with self.subTest(events=events):
                with self.assertRaisesRegex(example.SearchVerificationError, "Observed:"):
                    example.verify_search_flow(events)

    def test_cli_reports_verification_failure_without_traceback(self):
        example = load_example()
        with (
            patch.object(
                example, "run", AsyncMock(side_effect=example.SearchVerificationError("no search"))
            ),
            redirect_stdout(io.StringIO()),
            redirect_stderr(io.StringIO()) as error,
            self.assertRaises(SystemExit) as exit_result,
        ):
            example.cli([])
        self.assertEqual(exit_result.exception.code, 1)
        self.assertEqual(error.getvalue(), "Verification failed: no search\n")

    async def test_selected_tool_preserves_failures_and_does_not_register_fetch(self):
        async with SearchServer() as server:
            tool = WebSearch(
                backend=ExaSearch(
                    endpoint=server.origin + "/tool-error", private_origins=[server.origin]
                )
            )
            definitions, handler = extract_tools([tool])
            self.assertEqual([d.name for d in definitions], ["web_search"])
            with self.assertRaises(UnservedToolCallError):
                await handler("web_fetch", {"url": "https://example.com"})
            with self.assertRaises(ToolRefusedError):
                await handler("web_search", {"query": ""})
            with self.assertRaises(ToolFailedError):
                await handler("web_search", {"query": "q"})
