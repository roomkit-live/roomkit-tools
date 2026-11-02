"""Tests of scripts/api-parity.py, on small surfaces and snapshots built here.

Each test builds the Rust items and the Python specs a rule reads, and checks
the findings by key: `<side> <path> (<kind>)`.
"""

from __future__ import annotations

import importlib.util
import io
import json
import sys
import tempfile
import tomllib
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))

from api_parity import ParityError  # noqa: E402
from api_parity.cover import compare  # noqa: E402
from api_parity.mapping import Mapping  # noqa: E402
from api_parity.sides import Python, Surface  # noqa: E402

RULES = """
[rules]
acronyms = { Io = "IO" }
options_suffixes = ["Limits", "Config", "Policy"]
input_suffix = "Input"
arguments = "arguments"
cancellation = "CancellationToken"
factory = "create"
builder_prefix = "with_"
tool_protocol = { trait = "Tool", members = ["definition", "handler"] }
{extra}
[rules.packages]
kit = ["pkg"]
kit_adapter = ["pkg.adapter", "pkg"]
"""
BUILTINS = 'builtins = { "kit::Output" = "dict" }'
ERRORS = 'builtin_errors = { "kit::KitError::Cancelled" = "asyncio.CancelledError" }'


def mapping_text(entries: str = "", extra: str = "") -> str:
    return RULES.replace("{extra}", extra) + entries


CANCEL = ("cancel", "&CancellationToken")


def method(name, *params, receiver="&self", returns=None, is_async=False):
    return {
        "name": name,
        "async": is_async,
        "receiver": receiver,
        "params": [{"name": n, "type": t} for n, t in params],
        "returns": returns,
        "where": None,
    }


def struct(path, fields=(), methods=(), impls=(), non_exhaustive=False):
    return {
        "path": path,
        "kind": "struct",
        "decl": "",
        "non_exhaustive": non_exhaustive,
        "methods": list(methods),
        "constants": [],
        "impls": list(impls),
        "fields": [{"name": n, "type": t} for n, t in fields],
    }


def enum(path, variants, impls=(), methods=()):
    found = []
    for name, fields in variants:
        decl = f"{name} {{ ... }}" if fields else name
        found.append(
            {"name": name, "decl": decl, "fields": [{"name": f, "type": "String"} for f in fields]}
        )
    return {
        "path": path,
        "kind": "enum",
        "decl": "",
        "non_exhaustive": False,
        "methods": list(methods),
        "constants": [],
        "impls": list(impls),
        "variants": found,
    }


def trait(path, required=(), provided=()):
    return {
        "path": path,
        "kind": "trait",
        "decl": "",
        "non_exhaustive": False,
        "methods": [],
        "constants": [],
        "impls": [],
        "required": list(required),
        "provided": list(provided),
        "types": [],
    }


def function(path, *params, returns=None):
    return {
        "path": path,
        "kind": "fn",
        "decl": "",
        "non_exhaustive": False,
        "signature": method(path.rpartition("::")[2], *params, receiver=None, returns=returns),
    }


def run(items, specs, *, contracts=None, mapping="", rules="", rules_only=False, exports=None):
    crates: dict[str, list] = {}
    for item in items:
        crates.setdefault(item["path"].split("::")[0], []).append(item)
    surface = Surface([{"rustdoc": "test", "crates": crates}])
    if exports is None:
        exports = {}
        for path in specs:
            module, _, name = path.rpartition(".")
            exports.setdefault(module, []).append(name)
    python = Python(specs, exports, contracts)
    table = tomllib.loads(mapping_text(mapping, rules))
    return compare(surface, python, Mapping.from_table(table, rules_only))


def keys(report) -> set[str]:
    return {finding.key for finding in report.findings}


def uncovered(report) -> set[str]:
    return {finding.key for finding in report.uncovered}


def read_tool(*, limits=(("max_lines", "usize"),), input_new=True, extra_methods=()):
    """A standalone tool as the crates write one, and its Rust pieces."""
    tool = struct(
        "kit::ReadTool",
        methods=[
            method(
                "new",
                ("root", "impl AsRef<Path>"),
                receiver=None,
                returns="Result<Self, Error>",
                is_async=True,
            ),
            method(
                "with_limits",
                ("limits", "ReadLimits"),
                receiver="self",
                returns="Result<Self, Error>",
            ),
            method("definition", receiver=None, returns="Definition"),
            method(
                "read",
                ("input", "ReadInput"),
                CANCEL,
                returns="Result<Output<ReadPage>, Error>",
                is_async=True,
            ),
            *extra_methods,
        ],
    )
    options = struct("kit::ReadLimits", fields=limits, impls=["Default"])
    new = (
        [method("new", ("path", "impl Into<String>"), receiver=None, returns="Self")]
        if input_new
        else []
    )
    request = struct(
        "kit::ReadInput",
        fields=[("path", "String"), ("offset", "usize")],
        methods=new,
        impls=["JsonSchema"],
    )
    page = struct("kit::ReadPage", fields=[("lines", "Vec<String>")], impls=["JsonSchema"])
    return [tool, options, request, page]


READ_SPEC = {
    "pkg.ReadTool": {
        "__init__": ["def", "native"],
        "create": ["async classmethod", "root", "*", "max_lines=None"],
        "definition": ["staticmethod"],
        "read": ["async def", "path", "*", "offset=1"],
    }
}
READ_CONTRACTS = {
    "tools": {"read": {"class": "pkg.ReadTool", "input": "ReadInput", "output": "ReadPage"}},
    "types": {"ReadInput": {"fields": ["offset", "path"]}, "ReadPage": {"fields": ["lines"]}},
}


class Tools(unittest.TestCase):
    def test_a_factory_with_limits_and_a_call_with_its_input_match(self):
        report = run(read_tool(), READ_SPEC, contracts=READ_CONTRACTS)
        self.assertEqual(keys(report), set())

    def test_a_keyword_on_one_side_only(self):
        spec = {"pkg.ReadTool": dict(READ_SPEC["pkg.ReadTool"])}
        spec["pkg.ReadTool"]["create"] = [
            "async classmethod",
            "root",
            "*",
            "max_bytes=None",
            "max_depth=None",
        ]
        report = run(read_tool(), spec, contracts=READ_CONTRACTS)
        self.assertEqual(
            keys(report),
            {
                "python pkg.ReadTool.create(max_bytes) (unmatched)",
                "python pkg.ReadTool.create(max_depth) (unmatched)",
                "rust kit::ReadLimits::max_lines (unmatched)",
            },
        )

    def test_python_defaults_what_rust_requires(self):
        items = read_tool(input_new=False)
        report = run(items, READ_SPEC, contracts=READ_CONTRACTS)
        self.assertIn("rust kit::ReadInput::offset (defaulted)", keys(report))

    def test_python_requires_what_rust_defaults(self):
        items = read_tool(input_new=False)
        items[2]["impls"].append("Default")
        report = run(items, READ_SPEC, contracts=READ_CONTRACTS)
        self.assertEqual(keys(report), {"rust kit::ReadInput::path (required)"})

    def test_async_on_one_side_only(self):
        spec = {"pkg.ReadTool": dict(READ_SPEC["pkg.ReadTool"])}
        spec["pkg.ReadTool"]["read"] = ["def", "path", "*", "offset=1"]
        report = run(read_tool(), spec, contracts=READ_CONTRACTS)
        self.assertEqual(keys(report), {"rust kit::ReadTool::read (async)"})

    def test_an_options_struct_keeps_no_non_exhaustive(self):
        items = read_tool()
        items[1]["non_exhaustive"] = True
        report = run(items, READ_SPEC, contracts=READ_CONTRACTS)
        self.assertEqual(keys(report), {"rust kit::ReadLimits (non_exhaustive)"})

    def test_a_member_on_one_side_only(self):
        spec = {"pkg.ReadTool": {**READ_SPEC["pkg.ReadTool"], "call": ["async def", "arguments"]}}
        extra = [
            method("read_owned", ("owner", "&str"), ("input", "ReadInput"), CANCEL, is_async=True)
        ]
        report = run(read_tool(extra_methods=extra), spec, contracts=READ_CONTRACTS)
        self.assertEqual(
            keys(report),
            {"python pkg.ReadTool.call (unmatched)", "rust kit::ReadTool::read_owned (unmatched)"},
        )

    def test_a_call_returning_another_type_than_its_contract(self):
        contracts = json.loads(json.dumps(READ_CONTRACTS))
        contracts["tools"]["read"]["output"] = "ReadResult"
        contracts["types"]["ReadResult"] = {"fields": ["lines"]}
        report = run(read_tool(), READ_SPEC, contracts=contracts)
        self.assertIn("rust kit::ReadTool::read (result)", keys(report))

    def test_the_cancellation_token_and_self_are_no_parameter(self):
        report = run(read_tool(), READ_SPEC, contracts=READ_CONTRACTS)
        self.assertNotIn("rust kit::ReadTool::read(cancel) (unmatched)", keys(report))

    def test_python_call_is_the_json_form_of_the_typed_method(self):
        """`[rules] typed_call`: `call(arguments)` stands for the typed
        method taking the `<X>Input`, the one Python also names."""
        spec = {"pkg.ReadTool": {**READ_SPEC["pkg.ReadTool"], "call": ["async def", "arguments"]}}
        owned = method(
            "read_owned",
            ("owner", "&str"),
            ("input", "ReadInput"),
            CANCEL,
            returns="Result<Output<ReadPage>, Error>",
            is_async=True,
        )
        items = read_tool(extra_methods=[owned])
        rule = 'typed_call = { method = "call", decision = "ref" }'
        report = run(items, spec, contracts=READ_CONTRACTS, rules=rule)
        self.assertEqual(keys(report), {"rust kit::ReadTool::read_owned (unmatched)"})
        self.assertIn(
            "python pkg.ReadTool.call (unmatched)",
            keys(run(items, spec, contracts=READ_CONTRACTS)),
        )


SESSION_ITEMS = [
    struct(
        "kit::Session",
        methods=[
            method("new", receiver=None, returns="Self"),
            method("start_definition", receiver=None, returns="Definition"),
            method("stop_definition", receiver=None, returns="Definition"),
            *(
                method(
                    name,
                    ("owner", "Option<&str>"),
                    ("input", f"{name.title()}Input"),
                    CANCEL,
                    returns="Result<Output<Job>, Error>",
                    is_async=True,
                )
                for name in ("start", "stop")
            ),
        ],
    ),
    *(
        struct(
            f"kit::{name}Input",
            fields=[(field, "String")],
            methods=[method("new", (field, "String"), receiver=None, returns="Self")],
            impls=["JsonSchema"],
        )
        for name, field in (("Start", "command"), ("Stop", "id"))
    ),
    struct("kit::Job", fields=[("id", "String")], impls=["JsonSchema"]),
]
SESSION_SPECS = {
    "pkg.Session": {
        "__init__": ["def"],
        "call": ["async def", "name", "arguments", "*", "owner=None"],
        "definition": ["staticmethod", "name"],
        "start": ["async def", "command", "*", "owner=None"],
        "stop": ["async def", "id", "*", "owner=None"],
    }
}
SESSION_CONTRACTS = {
    "tools": {
        tool: {"class": "pkg.Session", "input": f"{tool.title()}Input", "output": "Job"}
        for tool in ("start", "stop")
    },
    "types": {
        "StartInput": {"fields": ["command"]},
        "StopInput": {"fields": ["id"]},
        "Job": {"fields": ["id"]},
    },
}
OPTIONAL = 'optional = { type = "Option", default = "None", decision = "ref" }\n'


class NamedCalls(unittest.TestCase):
    def test_a_name_keyed_call_and_definition_stand_for_each_typed_method(self):
        """`[rules] named_calls`: `call(name, arguments, ...)` stands for
        each typed method, and the static `definition(name)` for each
        `<x>_definition()`."""
        rule = (
            'named_calls = { method = "call", key = "name", definition = "definition", '
            'suffix = "_definition", decision = "ref" }'
        )
        report = run(
            SESSION_ITEMS, SESSION_SPECS, contracts=SESSION_CONTRACTS, rules=OPTIONAL + rule
        )
        self.assertEqual(keys(report), set())
        without = run(SESSION_ITEMS, SESSION_SPECS, contracts=SESSION_CONTRACTS, rules=OPTIONAL)
        self.assertEqual(
            keys(without),
            {
                "python pkg.Session.call (unmatched)",
                "python pkg.Session.definition (unmatched)",
                "rust kit::Session::start_definition (unmatched)",
                "rust kit::Session::stop_definition (unmatched)",
            },
        )


class Parameters(unittest.TestCase):
    def test_arguments_stand_for_the_input_struct_whole(self):
        items = [
            struct(
                "kit::DocsTool",
                methods=[
                    method("new", ("backend", "Arc<dyn Backend>"), receiver=None, returns="Self"),
                    method(
                        "call",
                        ("input", "DocsInput"),
                        CANCEL,
                        returns="Result<Output<Docs>, Error>",
                        is_async=True,
                    ),
                ],
            ),
            struct("kit::DocsInput", fields=[("offset", "usize")], impls=["JsonSchema"]),
        ]
        spec = {
            "pkg.DocsTool": {"__init__": ["def", "backend"], "call": ["async def", "arguments"]}
        }
        report = run(
            items, spec, contracts={"tools": {}, "types": {"DocsInput": {"fields": ["offset"]}}}
        )
        self.assertEqual(keys(report), set())

    def test_python_keywords_stand_for_named_rust_fields(self):
        items = [
            struct(
                "kit::FileDocs",
                methods=[
                    method(
                        "new",
                        ("root", "&Path"),
                        ("limits", "FileLimits"),
                        receiver=None,
                        returns="Self",
                    )
                ],
            ),
            struct(
                "kit::FileLimits",
                fields=[("max_documents", "usize"), ("max_depth", "usize")],
                impls=["Default"],
            ),
        ]
        spec = {"pkg.FileDocs": {"__init__": ["def", "root", "**limits"]}}
        report = run(items, spec)
        self.assertEqual(keys(report), {"python pkg.FileDocs.__init__(limits) (variadic)"})
        (finding,) = report.findings
        self.assertIn("max_documents, max_depth", finding.detail)

    def test_a_nested_options_struct_python_takes_flat(self):
        items = [
            struct(
                "kit::Search",
                methods=[method("new", ("config", "SearchConfig"), receiver=None, returns="Self")],
            ),
            struct(
                "kit::SearchConfig",
                fields=[("endpoint", "String"), ("policy", "NetworkPolicy")],
                impls=["Default"],
            ),
            struct(
                "kit::NetworkPolicy",
                fields=[("allowed", "Vec<String>"), ("private", "Vec<String>")],
                impls=["Default"],
            ),
        ]
        spec = {
            "pkg.Search": {
                "__init__": ["def", "*", "endpoint=None", "allowed=None", "private=None"]
            }
        }
        report = run(items, spec)
        self.assertEqual(keys(report), {"rust kit::SearchConfig::policy (form)"})

    def test_one_parameter_left_on_each_side_is_one_named_twice(self):
        items = [
            struct(
                "kit::Tool",
                methods=[method("new", ("manager", "Manager"), receiver=None, returns="Self")],
            )
        ]
        spec = {"pkg.Tool": {"__init__": ["def", "session"]}}
        report = run(items, spec)
        self.assertEqual(keys(report), {"rust kit::Tool::new(manager) (name)"})

    def test_an_option_stands_for_pythons_none_default(self):
        """`[rules] optional`: Rust has no optional call arguments; an
        `Option<T>` parameter is Python's `=None`, and no other default."""
        items = [
            struct(
                "kit::Store",
                methods=[
                    method("new", ("parent", "Option<&Path>"), receiver=None, returns="Self"),
                    method("list", ("owner", "Option<&str>"), is_async=True),
                    method("forget", ("owner", "Option<&str>"), is_async=True),
                ],
            )
        ]
        specs = {
            "pkg.Store": {
                "__init__": ["def", "parent=None"],
                "list": ["async def", "*", "owner=None"],
                "forget": ["async def", "*", "owner='me'"],
            }
        }
        rule = 'optional = { type = "Option", default = "None", decision = "ref" }'
        self.assertEqual(
            keys(run(items, specs, rules=rule)), {"rust kit::Store::forget(owner) (defaulted)"}
        )
        self.assertEqual(
            keys(run(items, specs)),
            {
                "rust kit::Store::new(parent) (defaulted)",
                "rust kit::Store::list(owner) (defaulted)",
                "rust kit::Store::forget(owner) (defaulted)",
            },
        )

    def test_a_trait_against_a_protocol_compares_no_default(self):
        items = [
            trait(
                "kit::Backend",
                required=[
                    method(
                        "list",
                        ("input", "&ListInput"),
                        CANCEL,
                        returns="BackendFuture<'a, Listing>",
                    )
                ],
            ),
            struct("kit::ListInput", fields=[("offset", "usize")], impls=["Default", "JsonSchema"]),
            {
                "path": "kit::BackendFuture",
                "kind": "type",
                "decl": "",
                "non_exhaustive": False,
                "aliased": "Pin<Box<dyn Future<Output = Result<T, Error>> + Send + 'a>>",
            },
        ]
        spec = {"pkg.Backend": {"list": ["async def", "*", "offset"]}}
        report = run(
            items, spec, contracts={"tools": {}, "types": {"ListInput": {"fields": ["offset"]}}}
        )
        self.assertEqual(keys(report), set())


class Members(unittest.TestCase):
    def test_a_property_is_a_method_of_self_alone(self):
        items = [
            struct(
                "kit::Store",
                methods=[
                    method("new", receiver=None, returns="Self"),
                    method("closed", returns="bool"),
                    method("size", ("unit", "Unit"), returns="usize"),
                ],
            )
        ]
        spec = {"pkg.Store": {"__init__": ["def"], "closed": ["property"], "size": ["property"]}}
        report = run(items, spec)
        self.assertEqual(keys(report), {"rust kit::Store::size (form)"})

    def test_a_static_method_is_an_associated_function(self):
        items = [
            struct(
                "kit::Tool",
                methods=[method("new", receiver=None, returns="Self"), method("definition")],
            )
        ]
        spec = {"pkg.Tool": {"__init__": ["def"], "definition": ["staticmethod"]}}
        report = run(items, spec)
        self.assertEqual(keys(report), {"rust kit::Tool::definition (form)"})

    def test_a_model_is_built_from_its_fields(self):
        items = [struct("kit::Scope", fields=[("room_id", "String"), ("caller_id", "String")])]
        report = run(items, {"pkg.Scope": {"__init__": ["def", "room_id", "caller_id"]}})
        self.assertEqual(keys(report), set())

    def test_a_constructor_building_another_type(self):
        items = [
            struct(
                "kit::Backend",
                methods=[method("new", receiver=None, returns="Result<Engine, Error>")],
            )
        ]
        report = run(items, {"pkg.Backend": {"__init__": ["def"]}})
        self.assertEqual(keys(report), {"rust kit::Backend::new (result)"})

    LAZY = 'lazy_constructor = { python = "__init__", decision = "ref" }\n'
    UNION = 'union_constructor = { prefix = "from_", decision = "ref" }\n'
    ROOTED = method(
        "new", ("root", "impl AsRef<Path>"), receiver=None, returns="Result<Self, E>", is_async=True
    )

    def test_a_lazy_constructor_stands_for_an_async_new(self):
        """`[rules] lazy_constructor`: Python's synchronous `__init__`
        resolves the root at first use, Rust's async `new` at once."""
        items = [struct("kit::Session", methods=[self.ROOTED])]
        specs = {"pkg.Session": {"__init__": ["def", "root"]}}
        self.assertEqual(keys(run(items, specs, rules=self.LAZY)), set())
        self.assertEqual(keys(run(items, specs)), {"rust kit::Session::new (async)"})

    def test_from_a_paired_type_is_the_constructors_union_parameter(self):
        """`[rules] union_constructor`: Python's `create(root | Workspace)`
        is Rust's `new(root)` and `from_workspace(workspace)`."""
        from_workspace = method(
            "from_workspace", ("workspace", "Workspace"), receiver=None, returns="Self"
        )
        items = [
            struct("kit::EditTool", methods=[self.ROOTED, from_workspace]),
            struct("kit::Workspace", methods=[self.ROOTED]),
        ]
        specs = {
            "pkg.EditTool": {
                "__init__": ["def", "native"],
                "create": ["async classmethod", "root"],
            },
            "pkg.Workspace": {"__init__": ["def", "root"]},
        }
        self.assertEqual(keys(run(items, specs, rules=self.LAZY + self.UNION)), set())
        self.assertEqual(
            keys(run(items, specs, rules=self.LAZY)),
            {"rust kit::EditTool::from_workspace (unmatched)"},
        )

    def test_items_and_names_on_one_side_only(self):
        items = [function("kit::helper", ("value", "&str"))]
        report = run(items, {"pkg.Extra": {"__init__": ["def"]}})
        self.assertEqual(
            keys(report), {"rust kit::helper (unmatched)", "python pkg.Extra (unmatched)"}
        )

    def test_a_type_alias_of_a_future_has_no_python_name(self):
        alias = {
            "path": "kit::ToolFuture",
            "kind": "type",
            "decl": "",
            "non_exhaustive": False,
            "aliased": "Pin<Box<dyn Future<Output = T> + Send>>",
        }
        report = run([alias], {"pkg.Other": {}}, exports={"pkg": ["Other"]})
        self.assertNotIn("rust kit::ToolFuture (unmatched)", keys(report))

    def test_a_builtin_member_is_a_finding(self):
        items = [
            struct("kit::Output", fields=[("data", "T")], methods=[method("new", receiver=None)])
        ]
        report = run(items, {"pkg.Other": {}}, rules=BUILTINS)
        self.assertIn("rust kit::Output::new (unmatched)", keys(report))


class ContractTypes(unittest.TestCase):
    def test_a_struct_whose_json_is_flattened(self):
        items = [
            struct(
                "kit::SearchResult",
                fields=[("query", "String"), ("response", "Backend")],
                impls=["JsonSchema"],
            ),
        ]
        contracts = {"tools": {}, "types": {"SearchResult": {"fields": ["payload", "query"]}}}
        report = run(items, {"pkg.Other": {}}, contracts=contracts)
        self.assertEqual(
            keys(report) - {"python pkg.Other (unmatched)"},
            {
                "rust kit::SearchResult::response (unmatched)",
                "python json:SearchResult.payload (unmatched)",
            },
        )

    def test_unit_variants_are_values_and_a_value_no_identifier_matches_by_position(self):
        items = [enum("kit::Mime", [("Png", ()), ("Jpeg", ())], impls=["JsonSchema"])]
        contracts = {"tools": {}, "types": {"Mime": {"values": ["image/png", "image/jpeg"]}}}
        report = run(items, {"pkg.Other": {}}, contracts=contracts)
        self.assertEqual(keys(report), {"python pkg.Other (unmatched)"})

    def test_tagged_variants_and_their_fields(self):
        items = [
            enum(
                "kit::Change",
                [("Add", ("path",)), ("Delete", ("path", "old"))],
                impls=["JsonSchema"],
            )
        ]
        shape = {"tag": "op", "variants": {"add": ["path"], "delete": ["path"], "move": ["to"]}}
        report = run(items, {"pkg.Other": {}}, contracts={"tools": {}, "types": {"Change": shape}})
        self.assertEqual(
            keys(report) - {"python pkg.Other (unmatched)"},
            {"rust kit::Change::Delete::old (unmatched)", "python json:Change.move (unmatched)"},
        )

    def test_a_contract_type_no_contract_holds(self):
        items = [struct("kit::Capabilities", fields=[("kinds", "Vec<Kind>")], impls=["JsonSchema"])]
        report = run(items, {"pkg.Other": {}})
        self.assertIn("rust kit::Capabilities (uncompared)", keys(report))

    def test_a_contract_type_named_as_a_python_class_of_another_meaning(self):
        items = [struct("kit::Status", fields=[("state", "State")], impls=["JsonSchema"])]
        contracts = {"tools": {}, "types": {"Status": {"fields": ["state"]}}}
        report = run(items, {"pkg.Status": {"__init__": ["def", "session"]}}, contracts=contracts)
        self.assertIn("rust kit::Status (meaning)", keys(report))


class Errors(unittest.TestCase):
    ITEMS = [
        enum(
            "kit::KitError",
            [("InvalidInput", ()), ("Refused", ()), ("Io", ()), ("Cancelled", ())],
            impls=["Debug", "Display", "Error"],
        )
    ]
    SPECS = {
        "pkg.KitError": {},
        "pkg.InvalidInputError": {"__bases__": ["bases", "pkg.KitError"]},
        "pkg.KitIOError": {"__bases__": ["bases", "pkg.KitError"]},
        "pkg.AccessRefusedError": {"__bases__": ["bases", "pkg.KitError"]},
    }

    def test_each_variant_is_a_class_of_the_family(self):
        report = run(self.ITEMS, self.SPECS, rules=ERRORS)
        self.assertEqual(
            keys(report),
            {
                "rust kit::KitError::Refused (unmatched)",
                "python pkg.AccessRefusedError (unmatched)",
            },
        )

    def test_a_builtin_error_the_rules_name_and_no_enum_has(self):
        items = [enum("kit::KitError", [("InvalidInput", ())], impls=["Error"])]
        specs = {
            "pkg.KitError": {},
            "pkg.InvalidInputError": {"__bases__": ["bases", "pkg.KitError"]},
        }
        entry = '[[pending]]\nreason = "r"\ncovers = ["rust kit::KitError::Cancelled (rules)"]\n'
        report = run(items, specs, mapping=entry, rules=ERRORS)
        self.assertIn("rust kit::KitError::Cancelled (rules)", uncovered(report))


class ToolProtocol(unittest.TestCase):
    def test_an_adapter_is_the_roomkit_tool_of_its_name(self):
        """A Rust adapter implementing the engine's `Tool` is compared with
        the Python RoomKit tool of its name: its `definition` and `handler`
        are the trait's, its constructor takes the built tool."""
        items = read_tool() + [
            struct(
                "kit_adapter::Read",
                methods=[method("new", ("tool", "ReadTool"), receiver=None, returns="Self")],
                impls=["Clone", "Debug", "Tool"],
            )
        ]
        specs = {
            **READ_SPEC,
            "pkg.adapter.Read": {
                "__init__": ["def", "tool"],
                "definition": ["property"],
                "handler": ["async def", "name", "arguments"],
            },
        }
        self.assertEqual(keys(run(items, specs, contracts=READ_CONTRACTS)), set())
        specs["pkg.adapter.Read"]["__init__"] = ["def", "root", "*", "max_lines=None"]
        self.assertEqual(
            keys(run(items, specs, contracts=READ_CONTRACTS)),
            {"rust kit_adapter::Read::new(tool) (form)"},
        )

    def test_the_roomkit_protocol_without_a_toolset(self):
        items = [
            struct(
                "kit::Plan",
                methods=[method("new", ("backend", "B"), receiver=None, returns="Self")],
            )
        ]
        spec = {
            "pkg.Plan": {
                "__init__": ["def", "backend"],
                "handler": ["async def", "name", "arguments"],
            }
        }
        report = run(items, spec)
        self.assertEqual(keys(report), {"python pkg.Plan.handler (unmatched)"})


class Entries(unittest.TestCase):
    ITEMS = [
        struct(
            "kit::DocsListTool",
            methods=[
                method("new", ("backend", "B"), receiver=None, returns="Self"),
                method("execute", ("input", "ListInput"), CANCEL, is_async=True),
            ],
        ),
        struct("kit::ListInput", fields=[("offset", "usize")], impls=["JsonSchema"]),
    ]
    SPECS = {"pkg.DocsList": {"__init__": ["def", "backend"], "call": ["async def", "arguments"]}}
    CONTRACTS = {"tools": {}, "types": {"ListInput": {"fields": ["offset"]}}}
    PAIRS = """
[[pending]]
rust = "kit::DocsListTool"
python = "pkg.DocsList"
reason = "name"
covers = ["rust kit::DocsListTool (name)"]

[[pending]]
rust = "kit::DocsListTool::execute"
python = "pkg.DocsList.call"
reason = "name"
covers = ["rust kit::DocsListTool::execute (name)"]
"""

    def test_without_entries_the_items_are_on_one_side_only(self):
        report = run(self.ITEMS, self.SPECS, contracts=self.CONTRACTS)
        self.assertEqual(
            uncovered(report),
            {"rust kit::DocsListTool (unmatched)", "python pkg.DocsList (unmatched)"},
        )

    def test_a_pending_pairing_compares_the_members_and_covers_the_names(self):
        report = run(self.ITEMS, self.SPECS, contracts=self.CONTRACTS, mapping=self.PAIRS)
        self.assertEqual(uncovered(report), set())
        self.assertEqual(report.dead, [])
        self.assertEqual(len(report.pending), 2)
        self.assertTrue(report.passed(strict=False))
        self.assertFalse(report.passed(strict=True))

    def test_an_approved_map_pairs_without_a_finding(self):
        mapping = self.PAIRS.replace("[[pending]]", "[[map]]").replace(
            'reason = "name"', 'approved = 2026-11-01\nreason = "forced"'
        )
        mapping = "\n".join(line for line in mapping.splitlines() if not line.startswith("covers"))
        report = run(self.ITEMS, self.SPECS, contracts=self.CONTRACTS, mapping=mapping)
        self.assertEqual(keys(report), set())
        self.assertTrue(report.passed(strict=True))

    def test_an_approved_map_renames_parameters(self):
        items = [
            struct(
                "kit::Engine",
                methods=[
                    method("start", ("job", "&str"), is_async=True),
                    method("new", receiver=None, returns="Self"),
                ],
            )
        ]
        specs = {"pkg.Engine": {"__init__": ["def"], "launch": ["async def", "task"]}}
        mapping = """
[[map]]
rust = "kit::Engine::start"
python = "pkg.Engine.launch"
params = { job = "task" }
approved = 2026-11-01
reason = "forced"
"""
        self.assertEqual(keys(run(items, specs, mapping=mapping)), set())
        unrenamed = mapping.replace('params = { job = "task" }\n', "")
        self.assertEqual(
            keys(run(items, specs, mapping=unrenamed)),
            {"rust kit::Engine::start(job) (name)"},
        )

    def test_dead_entries(self):
        mapping = (
            self.PAIRS
            + """
[[pending]]
reason = "resolved since"
covers = ["python pkg.DocsList.call (unmatched)"]

[[pending]]
rust = "kit::Gone"
python = "pkg.DocsList"
reason = "gone"
covers = ["rust kit::Gone (name)"]
"""
        )
        report = run(self.ITEMS, self.SPECS, contracts=self.CONTRACTS, mapping=mapping)
        whys = sorted(dead.why for dead in report.dead)
        self.assertEqual(
            whys,
            [
                "covers nothing: no such finding",
                "names nothing of the rust side",
                "names nothing of the rust side",
            ],
        )
        self.assertFalse(report.passed(strict=False))

    def test_a_pairing_the_rules_make_is_dead(self):
        mapping = """
[[pending]]
rust = "kit::DocsListTool"
python = "pkg.DocsListTool"
reason = "same"
covers = ["rust kit::DocsListTool (name)"]
"""
        specs = {"pkg.DocsListTool": self.SPECS["pkg.DocsList"]}
        report = run(self.ITEMS, specs, contracts=self.CONTRACTS, mapping=mapping)
        self.assertIn(
            "pairs what the rules pair: the names are the same", [dead.why for dead in report.dead]
        )

    def test_malformed_entries_are_refused(self):
        for entry in (
            '[[pending]]\nreason = "r"\ncovers = ["kit::X"]\n',
            '[[pending]]\nreason = "r"\ncovers = ["rust kit::X (whatever)"]\n',
            '[[pending]]\nreason = "r"\ncovers = []\n',
            '[[pending]]\nrust = "kit::X"\nreason = "r"\ncovers = ["rust kit::X (name)"]\n',
            '[[exception]]\napproved = "soon"\nreason = "r"\ncovers = ["rust kit::X (name)"]\n',
            '[[pending]]\nreason = "r"\ncovers = ["rust kit::X (name)"]\nlot = 2\n',
            '[[exception]]\napproved = 2026-11-02\nreason = "r"\ndecision = ""\n'
            'covers = ["rust kit::X (name)"]\n',
            '[[missing]]\napproved = 2026-11-02\nreason = "r"\ncovers = ["rust kit::X (name)"]\n',
        ):
            with self.subTest(entry=entry), self.assertRaises(ParityError):
                Mapping.from_table(tomllib.loads(mapping_text(entry)))

    def test_an_entry_and_a_rule_name_their_decision(self):
        """`decision` names where an entry or a rule was decided."""
        pairs = self.PAIRS.replace("[[pending]]", "[[map]]").replace(
            'reason = "name"', 'approved = 2026-11-02\nreason = "r"\ndecision = "ref Q1"'
        )
        entries = "\n".join(line for line in pairs.splitlines() if not line.startswith("covers"))
        entries += (
            '\n[[exception]]\napproved = 2026-11-02\nreason = "r"\ndecision = "ref Q2"\n'
            'covers = ["rust kit::X (name)"]\n'
        )
        rules = RULES.replace('"handler"] }', '"handler"], decision = "ref Q3" }')
        mapping = Mapping.from_table(tomllib.loads(rules.replace("{extra}", "") + entries))
        self.assertEqual(
            [entry.decision for entry in mapping.entries], ["ref Q1", "ref Q1", "ref Q2"]
        )
        self.assertEqual(mapping.rules.decisions, {"tool_protocol": "ref Q3"})

    def test_a_feature_on_one_side_only_is_missing_not_excepted(self):
        """A `[[missing]]` entry covers what one side has and the other lacks."""
        missing = (
            '[[missing]]\napproved = 2026-11-02\nreason = "Rust only"\ndecision = "ref Q9"\n'
            'covers = ["rust kit::DocsListTool (unmatched)", "python pkg.DocsList (unmatched)"]\n'
        )
        report = run(self.ITEMS, self.SPECS, contracts=self.CONTRACTS, mapping=missing)
        self.assertEqual(uncovered(report), set())
        self.assertEqual(len(report.missing), 2)
        self.assertTrue(report.passed(strict=True))


class Command(unittest.TestCase):
    """The gate's exit status: 0 green, 1 on a finding or a dead entry, 2 on
    an input it cannot read."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.dir = Path(self._tmp.name)
        surface = {"rustdoc": "test", "crates": {"kit": Entries.ITEMS}}
        (self.dir / "surface.json").write_text(json.dumps(surface))
        snapshot = self.dir / "python"
        snapshot.mkdir()
        (snapshot / "public_api_signatures.json").write_text(json.dumps(Entries.SPECS))
        (snapshot / "public_api_names.json").write_text(json.dumps({"pkg": ["DocsList"]}))
        (snapshot / "public_api_contracts.json").write_text(json.dumps(Entries.CONTRACTS))

    def tearDown(self):
        self._tmp.cleanup()

    def _main(self, mapping: str, *flags: str) -> tuple[int, str]:
        (self.dir / "mapping.toml").write_text(mapping_text(mapping))
        out = io.StringIO()
        status = gate.main(
            [
                str(self.dir / "surface.json"),
                "--python",
                str(self.dir / "python"),
                "--mapping",
                str(self.dir / "mapping.toml"),
                *flags,
            ],
            out=out,
            log=io.StringIO(),
        )
        return status, out.getvalue()

    def test_green_with_every_finding_covered(self):
        status, out = self._main(Entries.PAIRS)
        self.assertEqual(status, 0, out)
        self.assertIn("Gate: passed.", out)

    def test_strict_fails_while_a_pending_entry_is_left(self):
        self.assertEqual(self._main(Entries.PAIRS, "--strict")[0], 1)

    def test_a_finding_without_an_entry_fails(self):
        status, out = self._main("")
        self.assertEqual(status, 1)
        self.assertIn("rust   kit::DocsListTool (unmatched): no Python name", out)

    def test_pending_prints_the_entries_the_mapping_lacks(self):
        status, out = self._main("", "--pending")
        self.assertEqual(status, 0)
        entries = tomllib.loads(out)["pending"]
        covered = {key for entry in entries for key in entry["covers"]}
        self.assertEqual(
            covered, {"rust kit::DocsListTool (unmatched)", "python pkg.DocsList (unmatched)"}
        )

    def test_an_input_it_cannot_read(self):
        self.assertEqual(self._main("[[nonsense]]\n")[0], 2)


def _load_gate():
    spec = importlib.util.spec_from_file_location("api_parity_gate", HERE.parent / "api-parity.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


gate = _load_gate()


if __name__ == "__main__":
    unittest.main()
