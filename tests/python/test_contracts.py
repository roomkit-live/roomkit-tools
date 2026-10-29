"""Compare installed Python tools against Rust using the same real fixtures."""

from __future__ import annotations

import asyncio
import json
import os
import tempfile
import unittest
from contextlib import nullcontext
from pathlib import Path
from unittest.mock import patch

from roomkit_tools import (
    AccessRefusedError,
    DiffTool,
    EditTool,
    GlobTool,
    GrepTool,
    InvalidInputError,
    LimitExceededError,
    ListTool,
    OutputStore,
    PatchTool,
    ProcessSession,
    ReadTool,
    ShellTool,
    ToolIOError,
    UnsupportedResourceError,
    Workspace,
    WriteTool,
    _native,
)

FIXTURE = Path(__file__).resolve().parents[1] / "fixtures/contracts.json"
TOOLS = {
    "read": ReadTool,
    "list": ListTool,
    "glob": GlobTool,
    "grep": GrepTool,
    "write": WriteTool,
    "edit": EditTool,
    "diff": DiffTool,
    "patch": PatchTool,
    "shell": ShellTool,
}
ERRORS = {
    InvalidInputError: "invalid_input",
    AccessRefusedError: "refused",
    LimitExceededError: "limit",
    UnsupportedResourceError: "unsupported",
    ToolIOError: "io",
    asyncio.CancelledError: "cancelled",
}


def pointer(value, path):
    for part in path.lstrip("/").split("/"):
        key = part.replace("~1", "/").replace("~0", "~")
        value = value[int(key)] if isinstance(value, list) else value[key]
    return value


def normalize(value, root: Path):
    if isinstance(value, str):
        return value.replace(str(root), "$ROOT")
    if isinstance(value, dict):
        return {key: normalize(item, root) for key, item in value.items()}
    if isinstance(value, list):
        return [normalize(item, root) for item in value]
    return value


def file_bytes(value):
    return value.encode("utf-8") if isinstance(value, str) else bytes(value)


def snapshot(root):
    return {str(p.relative_to(root)): list(p.read_bytes()) for p in root.rglob("*") if p.is_file()}


class SharedContractTests(unittest.IsolatedAsyncioTestCase):
    async def test_installed_results_match_rust_and_fixture_expectations(self) -> None:
        reference_path = os.getenv("ROOMKIT_CONTRACT_RESULTS")
        if not reference_path:
            self.skipTest("make python-check generates the Rust reference results")
        reference = json.loads(await asyncio.to_thread(Path(reference_path).read_text))
        fixture = json.loads(FIXTURE.read_text())
        self.assertEqual(reference["fixture"], fixture, "stale Rust fixtures")
        names = {step["tool"] for case in fixture["cases"] for step in case["steps"]}
        self.assertEqual(
            names,
            set(TOOLS)
            | {"process_start", "process_status", "process_read", "process_write", "process_stop"}
            | {"output_read", "output_forget"},
        )
        for case in fixture["cases"]:
            with (
                self.subTest(case=case["name"]),
                tempfile.TemporaryDirectory(prefix="tool contracts ") as directory,
            ):
                root = await asyncio.to_thread(Path(directory).resolve)
                observed = await self.run_case(root, case)
                # JSON comparison preserves boolean/number distinctions that
                # Python equality would otherwise collapse (True == 1).
                self.assertEqual(
                    json.dumps(observed, sort_keys=True),
                    json.dumps(reference["reports"][case["name"]], sort_keys=True),
                )

    async def run_case(self, root: Path, case):
        for name, content in case["files"].items():
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(file_bytes(content))
        workspace = Workspace(root)
        results = []
        store = OutputStore(root.parent, max_read_bytes=3)
        outputs = store if case.get("outputs") else None
        async with (
            store,
            ProcessSession(root, outputs=outputs, max_output_bytes=8, max_read_bytes=3) as session,
        ):
            for step in case["steps"]:
                if "wait" in step:
                    async with asyncio.timeout(5):
                        while True:
                            status = await session.status("p1")
                            if all(
                                pointer(status, key) == value for key, value in step["wait"].items()
                            ):
                                break
                            await asyncio.sleep(0.005)
                name = step["tool"]
                if name.startswith("output_"):
                    tool = store
                elif name.startswith("process_"):
                    tool = session
                else:
                    options = {"max_output_bytes": 8} if name == "shell" else {}
                    if name == "shell":
                        options["outputs"] = outputs
                    tool = await TOOLS[name].create(workspace, **options)
                # Use the same pre-cancelled native scope as the Rust fixture,
                # while exercising the public Python argument/result bridge.
                token = _native._Cancellation()
                if step.get("cancelled"):
                    token.cancel()
                scope = (
                    patch.object(_native, "_Cancellation", return_value=token)
                    if step.get("cancelled")
                    else nullcontext()
                )
                try:
                    with scope:
                        result = await (
                            tool.call(name, step["input"])
                            if tool is session or tool is store
                            else tool.call(step["input"])
                        )
                except (
                    InvalidInputError,
                    AccessRefusedError,
                    LimitExceededError,
                    UnsupportedResourceError,
                    ToolIOError,
                    asyncio.CancelledError,
                ) as error:
                    result = {"error": ERRORS[type(error)]}
                result = normalize(result, root)
                for key, value in step["expect"].items():
                    self.assertEqual(pointer(result, key), value, (case["name"], step))
                results.append(result)
        files = await asyncio.to_thread(snapshot, root)
        expected = {
            name: list(file_bytes(content))
            for name, content in case.get("files_after", case["files"]).items()
        }
        self.assertEqual(files, expected, "final file bytes")
        return {"results": results, "files": files}
