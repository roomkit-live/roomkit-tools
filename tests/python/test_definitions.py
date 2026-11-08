"""Every tool's input schema, as an integrator hands it to a model."""

from __future__ import annotations

import unittest

from roomkit_tools import OutputStore, ProcessSession, ShellTool
from roomkit_tools.agents import (
    AgentFollowupTool,
    AgentSendTool,
    CancelTaskTool,
    DelegateTaskTool,
    TaskStatusTool,
)
from roomkit_tools.code import CodeOutlineTool
from roomkit_tools.docs import DocsListTool, DocsReadTool, DocsSearchTool
from roomkit_tools.fs import (
    DiffTool,
    EditTool,
    GlobTool,
    GrepTool,
    ListTool,
    PatchTool,
    ReadTool,
    WriteTool,
)
from roomkit_tools.image import ViewImageTool
from roomkit_tools.ocr import OcrTool
from roomkit_tools.plans import (
    PlanCreateTool,
    PlanDeleteTool,
    PlanListTool,
    PlanReadTool,
    PlanUpdateTool,
)
from roomkit_tools.search import WebSearchTool
from roomkit_tools.web import WebFetchTool

NUMBER_WIDTHS = {
    "int",
    "int8",
    "int16",
    "int32",
    "int64",
    "uint",
    "uint8",
    "uint16",
    "uint32",
    "uint64",
    "float",
    "double",
}
ONE_SCHEMA = ("items", "additionalProperties", "not", "if", "then", "else", "contains")
SCHEMA_LISTS = ("anyOf", "oneOf", "allOf", "prefixItems")
SCHEMA_MAPS = ("properties", "patternProperties", "$defs")


def definitions() -> list[dict]:
    tools = (
        AgentFollowupTool,
        AgentSendTool,
        CancelTaskTool,
        CodeOutlineTool,
        DelegateTaskTool,
        DiffTool,
        DocsListTool,
        DocsReadTool,
        DocsSearchTool,
        EditTool,
        GlobTool,
        GrepTool,
        ListTool,
        OcrTool,
        PatchTool,
        PlanCreateTool,
        PlanDeleteTool,
        PlanListTool,
        PlanReadTool,
        PlanUpdateTool,
        ReadTool,
        ShellTool,
        TaskStatusTool,
        ViewImageTool,
        WebFetchTool,
        WebSearchTool,
        WriteTool,
    )
    named = [
        ProcessSession.definition(f"process_{name}")
        for name in ("start", "status", "read", "write", "stop")
    ]
    stored = [OutputStore.definition(f"output_{name}") for name in ("read", "forget")]
    return [tool.definition() for tool in tools] + named + stored


def schemas_in(schema):
    """Yield `schema` and every schema it holds; property names are no keywords."""
    if not isinstance(schema, dict):
        return
    yield schema
    for keyword in ONE_SCHEMA:
        yield from schemas_in(schema.get(keyword))
    for keyword in SCHEMA_LISTS:
        for held in schema.get(keyword, ()):
            yield from schemas_in(held)
    for keyword in SCHEMA_MAPS:
        for held in schema.get(keyword, {}).values():
            yield from schemas_in(held)


class ModelSchemaTest(unittest.TestCase):
    def test_no_schema_carries_what_only_describes_the_rust_type(self):
        for definition in definitions():
            with self.subTest(definition["name"]):
                schema = definition["input_schema"]
                for keyword in ("$schema", "title", "description"):
                    self.assertNotIn(keyword, schema)
                for held in schemas_in(schema):
                    self.assertNotIn(held.get("format"), NUMBER_WIDTHS)
                    self.assertFalse("default" in held and held["default"] is None, held)

    def test_a_valid_call_can_still_be_written_from_the_schema(self):
        schema = ReadTool.definition()["input_schema"]
        self.assertEqual(schema["required"], ["path"])
        self.assertIs(schema["additionalProperties"], False)
        self.assertEqual(
            schema["properties"]["offset"],
            {"type": "integer", "minimum": 1, "default": 1, "description": "First line to return."},
        )
        # An optional argument stays nullable: a model may send null for it.
        self.assertEqual(
            schema["properties"]["limit"],
            {
                "type": ["integer", "null"],
                "minimum": 1,
                "description": "Line count, capped by the host.",
            },
        )

    def test_a_property_named_like_a_keyword_is_an_argument(self):
        properties = WebFetchTool.definition()["input_schema"]["properties"]
        self.assertIn("format", properties)
        self.assertIn("title", PlanCreateTool.definition()["input_schema"]["properties"])


if __name__ == "__main__":
    unittest.main()
