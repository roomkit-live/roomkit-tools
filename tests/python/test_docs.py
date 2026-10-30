"""Documentation contracts, snapshot ownership and injected backends without RoomKit."""

import asyncio
import copy
import json
import os
import tempfile
import unittest
from pathlib import Path

from roomkit_tools import (
    AccessRefusedError,
    DocsList,
    DocsRead,
    DocsSearch,
    FileDocs,
    InvalidInputError,
    LimitExceededError,
    UnsupportedResourceError,
)

FIXTURE = Path(__file__).resolve().parents[1] / "fixtures/documentation.json"


class DocsTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.fixture = json.loads(FIXTURE.read_text())
        for name, content in self.fixture["files"].items():
            (self.root / name).write_text(content)
        self.backend = FileDocs(self.root, version="fixture-v1")

    async def test_complete_envelopes_match_rust(self):
        tools = {t.name: t(self.backend) for t in (DocsList, DocsSearch, DocsRead)}
        results = [
            await tools[case["tool"]].call(case["arguments"]) for case in self.fixture["cases"]
        ]
        self.assertEqual(results[0]["data"]["total_documents"], 3)
        self.assertEqual(results[2]["data"]["matches"][0]["line"], 2)
        self.assertEqual(results[4]["data"]["next_offset"], 3)
        self.assertEqual(results[5]["data"]["lines"], [])
        for result in results:
            self.assertEqual(json.loads(result["text"]), result["data"])
            self.assertNotIn(str(self.root), json.dumps(result))
        if reference := os.getenv("ROOMKIT_DOCS_RESULTS"):
            self.assertEqual(
                results, json.loads(await asyncio.to_thread(Path(reference).read_text))
            )

    async def test_snapshot_is_shared_but_refresh_is_explicit(self):
        listing = await DocsList(self.backend).call({})
        original = listing["data"]["documents"][0]
        (self.root / "agents.md").write_text("# Changed\nUpdated instructions\n")
        arguments = {
            "document_id": original["document_id"],
            "expected_revision": original["revision"],
        }
        old = await DocsRead(self.backend).call(arguments)
        self.assertEqual(old["data"]["lines"][0]["text"], "# Delegation")
        fresh = FileDocs(self.root, version="fixture-v1")
        with self.assertRaises(AccessRefusedError):
            await DocsRead(fresh).call(arguments)
        with self.assertRaises(AccessRefusedError):
            await DocsSearch(fresh).call(
                {
                    "query": "changed",
                    "expected_catalog_revision": listing["data"]["catalog_revision"],
                }
            )
        for path in ("../outside.md", str(self.root / "agents.md"), "https://example.com"):
            with self.assertRaises(AccessRefusedError):
                await DocsRead(self.backend).call({"document_id": path})

    async def test_literal_terms_ranking_and_bounded_whole_lines(self):
        (self.root / "rank.md").write_text("# agent_followup\nMatch a.b literally.\n" + "é" * 1500)
        backend = FileDocs(self.root, max_read_bytes=4096)
        search = DocsSearch(backend)
        hits = (await search.call({"query": "AGENT_FOLLOWUP"}))["data"]["matches"]
        self.assertEqual([h["document"]["document_id"] for h in hits], ["rank.md", "agents.md"])
        self.assertEqual((await search.call({"query": "a.b"}))["data"]["matched_documents"], 1)
        self.assertEqual((await search.call({"query": "a.*b"}))["data"]["matched_documents"], 0)
        page = (await DocsRead(backend).call({"document_id": "rank.md"}))["data"]
        self.assertFalse(page["truncated"])
        self.assertEqual(page["lines"][2]["text"], "é" * 1500)
        (self.root / "large.md").write_text("x" * 5000)
        with self.assertRaises(LimitExceededError):
            await DocsRead(FileDocs(self.root, max_read_bytes=4096)).call(
                {"document_id": "large.md"}
            )

    async def test_limits_invalid_inputs_and_retry_failed_load(self):
        for arguments in ({"query": " "}, {"query": "x", "limit": 0}, {"query": "x", "root": "/"}):
            with self.assertRaises(InvalidInputError):
                await DocsSearch(self.backend).call(arguments)
        with self.assertRaises(InvalidInputError):
            FileDocs(self.root, unknown_limit=3)
        with self.assertRaises(LimitExceededError):
            await DocsList(FileDocs(self.root, max_documents=1)).call({})
        bad = self.root / "bad.md"
        bad.write_bytes(b"\xff")
        with self.assertRaises(UnsupportedResourceError):
            await DocsList(self.backend).call({})
        bad.unlink()
        self.assertEqual((await DocsList(self.backend).call({}))["data"]["total_documents"], 3)

    async def test_custom_backend_is_validated_and_cancellable(self):
        data = await self.backend.list()
        started, cancelled = asyncio.Event(), asyncio.Event()

        class Custom:
            arguments = None

            async def list(self, **arguments):
                self.arguments = arguments
                return copy.deepcopy(data)

            async def read(self, **arguments):
                started.set()
                try:
                    await asyncio.Future()
                finally:
                    cancelled.set()

        backend = Custom()
        result = await DocsList(backend).call({})
        self.assertEqual(result["data"], data)
        self.assertEqual(
            backend.arguments, {"offset": 0, "limit": 20, "expected_catalog_revision": None}
        )
        with self.assertRaises(UnsupportedResourceError):
            await DocsSearch(backend).call({"query": "agent"})
        data["next_offset"] = 0
        with self.assertRaises(InvalidInputError):
            await DocsList(backend).call({})
        task = asyncio.create_task(DocsRead(backend).call({"document_id": "agents.md"}))
        await asyncio.wait_for(started.wait(), 2)
        task.cancel()
        with self.assertRaises(asyncio.CancelledError):
            await task
        self.assertTrue(cancelled.is_set())


class DocsLoopTests(unittest.TestCase):
    def test_relative_root_is_captured_and_snapshot_survives_loops(self):
        previous = Path.cwd()
        with tempfile.TemporaryDirectory() as temporary:
            try:
                os.chdir(temporary)
                Path("guide.md").write_text("# Guide\nReference\n")
                backend = FileDocs(".")
            finally:
                os.chdir(previous)
            first = asyncio.run(DocsList(backend).call({}))
            second = asyncio.run(DocsList(backend).call({}))
            self.assertEqual(first, second)
            self.assertEqual(first["data"]["documents"][0]["document_id"], "guide.md")
