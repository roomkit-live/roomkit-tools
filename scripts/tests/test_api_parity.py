"""Tests of scripts/api-parity.py, on small surfaces and snapshots built here.

Each test builds the Rust items and the Python specs a rule reads, and checks
the findings by key: `<side> <path> (<kind>)`.
"""

from __future__ import annotations

import contextlib
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
from api_parity.defaults import Defaults  # noqa: E402
from api_parity.mapping import Mapping  # noqa: E402
from api_parity.sides import Python, Surface  # noqa: E402

RULES = """
[rules]
acronyms = { Io = "IO", decision = "ref" }
receivers = { decision = "ref" }
results = { decision = "ref" }
asynchronous = { decision = "ref" }
factory = { method = "create", decision = "ref" }
builder_prefix = { prefix = "with_", decision = "ref" }
options_suffixes = { suffixes = ["Limits", "Config", "Policy"], decision = "ref" }
input_suffix = { suffix = "Input", decision = "ref" }
arguments = { name = "arguments", decision = "ref" }
cancellation = { type = "CancellationToken", decision = "ref" }
contract_types = { decision = "ref" }
properties = { decision = "ref" }
traits = { decision = "ref" }
error_families = { decision = "ref" }
tool_protocol = { trait = "Tool", members = ["definition", "handler"], decision = "ref" }
{extra}
[rules.packages]
decision = "ref"
kit = ["pkg"]
kit_adapter = ["pkg.adapter", "pkg"]
"""
BUILTINS = 'builtins = { "kit::Output" = "dict", decision = "ref" }'
ERRORS = (
    'builtin_errors = { "kit::KitError::Cancelled" = "asyncio.CancelledError", decision = "ref" }'
)


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


def run(
    items,
    specs,
    *,
    contracts=None,
    mapping="",
    rules="",
    rules_only=False,
    exports=None,
    defaults=None,
    hidden=None,
):
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
    dump = None if defaults is None else Defaults(defaults)
    return compare(surface, python, Mapping.from_table(table, rules_only), dump, hidden)


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
                "python pkg.ReadTool.create(max_bytes) (unmatched) [kit::ReadTool::new]",
                "python pkg.ReadTool.create(max_depth) (unmatched) [kit::ReadTool::new]",
                "rust kit::ReadLimits::max_lines (unmatched) [pkg.ReadTool.create]",
            },
        )

    def test_python_defaults_what_rust_requires(self):
        items = read_tool(input_new=False)
        report = run(items, READ_SPEC, contracts=READ_CONTRACTS)
        self.assertIn(
            "rust kit::ReadInput::offset (defaulted) [pkg.ReadTool.read(offset=1)]", keys(report)
        )

    def test_python_requires_what_rust_defaults(self):
        items = read_tool(input_new=False)
        items[2]["impls"].append("Default")
        report = run(items, READ_SPEC, contracts=READ_CONTRACTS)
        self.assertEqual(
            keys(report), {"rust kit::ReadInput::path (required) [pkg.ReadTool.read(path)]"}
        )

    def test_async_on_one_side_only(self):
        spec = {"pkg.ReadTool": dict(READ_SPEC["pkg.ReadTool"])}
        spec["pkg.ReadTool"]["read"] = ["def", "path", "*", "offset=1"]
        report = run(read_tool(), spec, contracts=READ_CONTRACTS)
        self.assertEqual(keys(report), {"rust kit::ReadTool::read (async) [pkg.ReadTool.read]"})

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
        self.assertIn(
            "rust kit::ReadTool::read (result) [read: ReadPage, contract ReadResult]", keys(report)
        )

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
    RULES = (
        OPTIONAL + 'named_calls = { method = "call", key = "name", definition = "definition", '
        'suffix = "_definition", decision = "ref" }'
    )

    @staticmethod
    def _session(*, drop=(), returns=None, extra=()):
        """SESSION_ITEMS with Rust members dropped, a call's output changed
        and members added."""
        items = json.loads(json.dumps(SESSION_ITEMS))
        methods = [m for m in items[0]["methods"] if m["name"] not in drop]
        for found in methods:
            if returns and found["name"] in returns:
                found["returns"] = f"Result<Output<{returns[found['name']]}>, Error>"
        items[0]["methods"] = methods + list(extra)
        return items

    def _keys(self, items, contracts=SESSION_CONTRACTS):
        return keys(run(items, SESSION_SPECS, contracts=contracts, rules=self.RULES))

    def test_a_typed_method_no_contract_of_the_class_takes_is_unmatched(self):
        restart = method(
            "restart",
            ("owner", "Option<&str>"),
            ("input", "RestartInput"),
            CANCEL,
            returns="Result<Output<Job>, Error>",
            is_async=True,
        )
        items = self._session(extra=[restart]) + [
            struct("kit::RestartInput", fields=[("id", "String")], impls=["JsonSchema"])
        ]
        self.assertIn("rust kit::Session::restart (unmatched)", self._keys(items))

    def test_each_contract_has_its_typed_method_and_its_definition(self):
        self.assertEqual(
            self._keys(self._session(drop=("stop_definition",))),
            {"python pkg.Session.definition (unmatched) [stop]"},
        )
        self.assertEqual(
            self._keys(self._session(drop=("stop", "stop_definition"))),
            {
                "python pkg.Session.call (unmatched) [stop]",
                "python pkg.Session.stop (unmatched)",
                "rust kit::StopInput::new (unmatched)",  # no call compared builds it
            },
        )

    def test_each_typed_method_returns_its_contracts_output(self):
        items = self._session(returns={"stop": "Other"})
        self.assertEqual(
            self._keys(items), {"rust kit::Session::stop (result) [stop: Other, contract Job]"}
        )

    def test_a_list_output_is_the_contracts_array(self):
        items = self._session(returns={"stop": "Vec<Job>"})
        contracts = json.loads(json.dumps(SESSION_CONTRACTS))
        contracts["tools"]["stop"]["output"] = "Array_of_Job"
        self.assertEqual(self._keys(items, contracts), set())

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
        self.assertEqual(
            keys(report),
            {
                "python pkg.FileDocs.__init__(limits) (variadic) "
                "[kit::FileDocs::new: FileLimits(max_documents, max_depth)]"
            },
        )
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
        self.assertEqual(
            keys(report),
            {
                "rust kit::SearchConfig::policy (form) "
                "[pkg.Search.__init__: NetworkPolicy(allowed, private)]"
            },
        )

    def test_one_parameter_left_on_each_side_is_one_named_twice(self):
        items = [
            struct(
                "kit::Tool",
                methods=[method("new", ("manager", "Manager"), receiver=None, returns="Self")],
            )
        ]
        spec = {"pkg.Tool": {"__init__": ["def", "session"]}}
        report = run(items, spec)
        self.assertEqual(
            keys(report), {"rust kit::Tool::new(manager) (name) [pkg.Tool.__init__(session)]"}
        )

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
        forget = "rust kit::Store::forget(owner) (defaulted) [pkg.Store.forget(owner='me')]"
        self.assertEqual(keys(run(items, specs, rules=rule)), {forget})
        self.assertEqual(
            keys(run(items, specs)),
            {
                "rust kit::Store::new(parent) (defaulted) [pkg.Store.__init__(parent=None)]",
                "rust kit::Store::list(owner) (defaulted) [pkg.Store.list(owner=None)]",
                forget,
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
        self.assertEqual(keys(report), {"rust kit::Store::size (form) [pkg.Store.size]"})

    def test_a_static_method_is_an_associated_function(self):
        items = [
            struct(
                "kit::Tool",
                methods=[method("new", receiver=None, returns="Self"), method("definition")],
            )
        ]
        spec = {"pkg.Tool": {"__init__": ["def"], "definition": ["staticmethod"]}}
        report = run(items, spec)
        self.assertEqual(keys(report), {"rust kit::Tool::definition (form) [pkg.Tool.definition]"})

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
    UNION = 'union_constructor = { prefix = "from_", types = ["Workspace"], decision = "ref" }\n'
    ROOTED = method(
        "new", ("root", "impl AsRef<Path>"), receiver=None, returns="Result<Self, E>", is_async=True
    )

    def test_a_lazy_constructor_stands_for_an_async_new(self):
        """`[rules] lazy_constructor`: Python's synchronous `__init__`
        resolves the root at first use, Rust's async `new` at once."""
        items = [struct("kit::Session", methods=[self.ROOTED])]
        specs = {"pkg.Session": {"__init__": ["def", "root"]}}
        self.assertEqual(keys(run(items, specs, rules=self.LAZY)), set())
        self.assertEqual(
            keys(run(items, specs)), {"rust kit::Session::new (async) [pkg.Session.__init__]"}
        )

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

    def test_a_union_takes_only_the_types_its_rule_names(self):
        from_store = method("from_store", ("store", "Store"), receiver=None, returns="Self")
        items = [
            struct("kit::EditTool", methods=[self.ROOTED, from_store]),
            struct("kit::Store", methods=[self.ROOTED]),
        ]
        specs = {
            "pkg.EditTool": {
                "__init__": ["def", "native"],
                "create": ["async classmethod", "root"],
            },
            "pkg.Store": {"__init__": ["def", "root"]},
        }
        self.assertEqual(
            keys(run(items, specs, rules=self.LAZY + self.UNION)),
            {"rust kit::EditTool::from_store (unmatched)"},
        )

    def test_a_factory_classs_init_is_compared_unless_it_wraps_the_native_object(self):
        """With an async factory, Python's `__init__` taking only the native
        object it built is not compared; one taking parameters is, as a
        constructor."""
        specs = {"pkg.ReadTool": dict(READ_SPEC["pkg.ReadTool"])}
        self.assertEqual(keys(run(read_tool(), specs, contracts=READ_CONTRACTS)), set())
        specs["pkg.ReadTool"]["__init__"] = ["def", "root", "*", "max_lines=None", "strict=False"]
        self.assertEqual(
            keys(run(read_tool(), specs, contracts=READ_CONTRACTS, rules=self.LAZY)),
            {"python pkg.ReadTool.__init__(strict) (unmatched) [kit::ReadTool::new]"},
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

    def test_an_associated_constant_and_its_value(self):
        items = [struct("kit::Kit", methods=[method("new", receiver=None, returns="Self")])]
        items[0]["constants"] = [
            {"name": "MAX", "type": "usize", "value": "1_024"},
            {"name": "NAME", "type": "&str", "value": '"kit"'},
            {"name": "SHAPE", "type": "Shape", "value": None},
        ]
        spec = {"pkg.Kit": {"__init__": ["def"], "MAX": ["member", "1024"]}}
        self.assertEqual(
            keys(run(items, spec)),
            {"rust kit::Kit::NAME (unmatched)", "rust kit::Kit::SHAPE (unmatched)"},
        )
        spec["pkg.Kit"].update(MAX=["member", "2048"], SHAPE=["member", "'round'"])
        spec["pkg.Kit"]["NAME"] = ["member", "'kit'"]
        self.assertEqual(
            keys(run(items, spec)),
            {
                "rust kit::Kit::MAX (value) [pkg.Kit.MAX=2048, Rust 1_024]",
                "rust kit::Kit::SHAPE (value) [pkg.Kit.SHAPE='round', Rust None]",
            },
        )

    def test_the_variants_of_an_enum_no_contract_holds(self):
        """An enum that is neither a contract type nor an error is a Python
        enum: each variant a member of its name."""
        items = [enum("kit::Mode", [("Fast", ()), ("Safe", ())])]
        spec = {"pkg.Mode": {"Fast": ["member", "1"], "Slow": ["member", "2"]}}
        self.assertEqual(
            keys(run(items, spec)),
            {"rust kit::Mode::Safe (unmatched)", "python pkg.Mode.Slow (unmatched)"},
        )

    def test_a_new_parameter_that_is_no_field(self):
        items = [
            struct(
                "kit::Tool",
                methods=[method("new", ("input", "ReadInput"), receiver=None, returns="Self")],
            ),
            struct(
                "kit::ReadInput",
                fields=[("path", "String")],
                methods=[
                    method(
                        "new", ("path", "String"), ("strict", "bool"), receiver=None, returns="Self"
                    )
                ],
                impls=["JsonSchema"],
            ),
        ]
        contracts = {"tools": {}, "types": {"ReadInput": {"fields": ["path"]}}}
        spec = {"pkg.Tool": {"__init__": ["def", "path"]}}
        self.assertEqual(
            keys(run(items, spec, contracts=contracts)),
            {"rust kit::ReadInput::new(strict) (unmatched) [pkg.Tool.__init__]"},
        )


class Traits(unittest.TestCase):
    """A Rust type implementing a trait of the surface has its members on
    its Python class, or on the Python base standing for that trait."""

    BACKEND = trait(
        "kit::SearchBackend",
        required=[
            method("capabilities", returns="Capabilities"),
            method("search", ("query", "&str"), is_async=True),
        ],
    )

    def test_a_trait_member_the_python_class_lacks(self):
        items = [
            struct(
                "kit::FileDocs",
                methods=[method("new", receiver=None, returns="Self")],
                impls=["DocsBackend"],
            ),
            trait(
                "kit::DocsBackend",
                required=[
                    method("list", ("offset", "usize"), is_async=True),
                    method("search", ("query", "&str"), is_async=True),
                ],
            ),
        ]
        specs = {
            "pkg.FileDocs": {"__init__": ["def"], "list": ["async def", "offset"]},
            "pkg.DocsBackend": {
                "list": ["async def", "offset"],
                "search": ["async def", "query"],
            },
        }
        self.assertEqual(keys(run(items, specs)), {"rust kit::FileDocs::search (unmatched)"})

    def test_a_member_its_base_holds_is_compared_on_the_trait(self):
        items = [
            self.BACKEND,
            struct(
                "kit::ExaSearch",
                methods=[method("new", receiver=None, returns="Self")],
                impls=["SearchBackend"],
            ),
        ]
        specs = {
            "pkg.SearchBackend": {"capabilities": ["property"]},
            "pkg.ExaSearch": {
                "__bases__": ["bases", "pkg.SearchBackend"],
                "__init__": ["def"],
                "capabilities": ["property"],
            },
        }
        # search is on neither: the trait's comparison reports it, once.
        self.assertEqual(keys(run(items, specs)), {"rust kit::SearchBackend::search (unmatched)"})

    def test_a_base_member_is_compared_on_a_class_not_implementing_its_trait(self):
        items = [
            self.BACKEND,
            struct("kit::ExaSearch", methods=[method("new", receiver=None, returns="Self")]),
        ]
        specs = {
            "pkg.SearchBackend": {"capabilities": ["property"], "search": ["async def", "query"]},
            "pkg.ExaSearch": {
                "__bases__": ["bases", "pkg.SearchBackend"],
                "__init__": ["def"],
                "capabilities": ["property"],
            },
        }
        self.assertEqual(keys(run(items, specs)), {"python pkg.ExaSearch.capabilities (unmatched)"})


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

    def test_a_member_of_a_family_class_is_a_finding(self):
        """An error variant has no member: a family class's own is a
        finding, but an `__init__` taking the message alone."""
        specs = json.loads(json.dumps(self.SPECS))
        specs["pkg.InvalidInputError"]["field"] = ["property"]
        specs["pkg.KitIOError"]["__init__"] = ["def", "message"]
        specs["pkg.KitError"]["code"] = ["def"]
        found = keys(run(self.ITEMS, specs, rules=ERRORS))
        self.assertEqual(
            found - keys(run(self.ITEMS, self.SPECS, rules=ERRORS)),
            {
                "python pkg.InvalidInputError.field (unmatched)",
                "python pkg.KitError.code (unmatched)",
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
            {
                "rust kit_adapter::Read::new(tool) (form) "
                "[pkg.adapter.Read.__init__(root, max_lines): ReadTool]"
            },
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
covers = ["rust kit::DocsListTool (name) [pkg.DocsList]"]

[[pending]]
rust = "kit::DocsListTool::execute"
python = "pkg.DocsList.call"
reason = "name"
covers = ["rust kit::DocsListTool::execute (name) [pkg.DocsList.call]"]
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
            'reason = "name"', 'approved = 2026-11-01\nreason = "forced"\ndecision = "ref"'
        )
        mapping = "\n".join(line for line in mapping.splitlines() if not line.startswith("covers"))
        report = run(self.ITEMS, self.SPECS, contracts=self.CONTRACTS, mapping=mapping)
        self.assertEqual(keys(report), set())
        self.assertTrue(report.passed(strict=True))

    def test_an_exception_naming_both_counterparts_pairs_them(self):
        """An approved difference that is no language-forced mapping pairs
        its counterparts in an `[[exception]]`, which covers the name."""
        exception = (
            '[[exception]]\nrust = "kit::DocsListTool::execute"\npython = "pkg.DocsList.call"\n'
            'approved = 2026-11-02\nreason = "release"\ndecision = "ref"\n'
            'covers = ["rust kit::DocsListTool::execute (name) [pkg.DocsList.call]"]\n'
        )
        mapping = self.PAIRS.split("\n\n[[pending]]")[0] + "\n" + exception
        report = run(self.ITEMS, self.SPECS, contracts=self.CONTRACTS, mapping=mapping)
        self.assertEqual(uncovered(report), set())
        self.assertEqual(report.dead, [])
        self.assertEqual(len(report.covered_by("exception")), 1)

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
decision = "ref"
"""
        self.assertEqual(keys(run(items, specs, mapping=mapping)), set())
        unrenamed = mapping.replace('params = { job = "task" }\n', "")
        self.assertEqual(
            keys(run(items, specs, mapping=unrenamed)),
            {"rust kit::Engine::start(job) (name) [pkg.Engine.launch(task)]"},
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
            '[[exception]]\napproved = "soon"\nreason = "r"\ndecision = "ref"\n'
            'covers = ["rust kit::X (name)"]\n',
            '[[pending]]\nreason = "r"\ncovers = ["rust kit::X (name)"]\nlot = 2\n',
            '[[exception]]\napproved = 2026-11-02\nreason = "r"\ndecision = ""\n'
            'covers = ["rust kit::X (name)"]\n',
            '[[missing]]\napproved = 2026-11-02\nreason = "r"\ndecision = "ref"\n'
            'covers = ["rust kit::X (name)"]\n',
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
        rules = RULES.replace('"handler"], decision = "ref" }', '"handler"], decision = "ref Q3" }')
        mapping = Mapping.from_table(tomllib.loads(rules.replace("{extra}", "") + entries))
        self.assertEqual(
            [entry.decision for entry in mapping.entries], ["ref Q1", "ref Q1", "ref Q2"]
        )
        self.assertEqual(mapping.rules.decisions["tool_protocol"], "ref Q3")
        self.assertEqual(set(mapping.rules.decisions.values()), {"ref", "ref Q3"})

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


def pending(*covers: str) -> str:
    """A `[[pending]]` entry covering these keys."""
    keys = ", ".join(f'"{key}"' for key in covers)
    return f'[[pending]]\nreason = "r"\ncovers = [{keys}]\n'


class Keys(unittest.TestCase):
    """A key names what its finding was compared with, so an entry covers
    what was approved and nothing more: a change makes it dead."""

    NAMED = (
        OPTIONAL + 'named_calls = { method = "call", key = "name", definition = "definition", '
        'suffix = "_definition", decision = "ref" }'
    )

    def test_a_python_parameter_names_the_rust_call_it_lacks(self):
        specs = json.loads(json.dumps(SESSION_SPECS))
        specs["pkg.Session"]["call"].append("verbose=False")
        start = "python pkg.Session.call(verbose) (unmatched) [kit::Session::start]"
        stop = "python pkg.Session.call(verbose) (unmatched) [kit::Session::stop]"
        # A key naming no Rust call covers neither.
        unnamed = pending("python pkg.Session.call(verbose) (unmatched)")
        report = run(
            SESSION_ITEMS, specs, contracts=SESSION_CONTRACTS, rules=self.NAMED, mapping=unnamed
        )
        self.assertEqual(uncovered(report), {start, stop})
        report = run(
            SESSION_ITEMS,
            specs,
            contracts=SESSION_CONTRACTS,
            rules=self.NAMED,
            mapping=pending(start),
        )
        self.assertEqual(uncovered(report), {stop})

    def test_a_parameter_named_twice_names_its_python_name(self):
        items = [
            struct(
                "kit::Tool",
                methods=[method("new", ("manager", "Manager"), receiver=None, returns="Self")],
            )
        ]
        entry = pending("rust kit::Tool::new(manager) (name) [pkg.Tool.__init__(session)]")
        report = run(items, {"pkg.Tool": {"__init__": ["def", "sess"]}}, mapping=entry)
        self.assertEqual(
            uncovered(report), {"rust kit::Tool::new(manager) (name) [pkg.Tool.__init__(sess)]"}
        )
        self.assertEqual([dead.why for dead in report.dead], ["covers nothing: no such finding"])

    def test_a_struct_that_grows_leaves_its_entry_dead(self):
        """`variadic` and nested `form` keys name the fields covered."""
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
        entry = pending(
            "python pkg.FileDocs.__init__(limits) (variadic) "
            "[kit::FileDocs::new: FileLimits(max_documents, max_depth)]"
        )
        self.assertEqual(uncovered(run(items, spec, mapping=entry)), set())
        items[1]["fields"].append({"name": "max_bytes", "type": "usize"})
        report = run(items, spec, mapping=entry)
        self.assertEqual(len(report.uncovered), 1)
        self.assertEqual(len(report.dead), 1)

    def test_a_wrapped_tool_names_the_parameters_python_takes(self):
        items = read_tool() + [
            struct(
                "kit_adapter::Read",
                methods=[method("new", ("tool", "ReadTool"), receiver=None, returns="Self")],
                impls=["Tool"],
            )
        ]
        specs = {
            **READ_SPEC,
            "pkg.adapter.Read": {
                "__init__": ["def", "root", "*", "max_lines=None"],
                "definition": ["property"],
                "handler": ["async def", "name", "arguments"],
            },
        }
        entry = pending(
            "rust kit_adapter::Read::new(tool) (form) "
            "[pkg.adapter.Read.__init__(root, max_lines): ReadTool]"
        )
        report = run(items, specs, contracts=READ_CONTRACTS, mapping=entry)
        self.assertEqual(uncovered(report), set())
        specs["pkg.adapter.Read"]["__init__"] = ["def", "root"]
        report = run(items, specs, contracts=READ_CONTRACTS, mapping=entry)
        self.assertEqual(len(report.dead), 1)

    def test_a_default_names_both_values(self):
        items, spec, contracts = DefaultValues._limit("usize", "20")
        dump = {**DefaultValues.DUMP, "kit::ReadInput": {"path": "a", "offset": 1, "limit": 10}}
        entry = pending(
            "rust kit::ReadInput::limit (default) [pkg.ReadTool.read(limit=20), Rust 10]"
        )
        rules = DefaultValues.DEFERRED
        report = run(items, spec, contracts=contracts, rules=rules, mapping=entry, defaults=dump)
        self.assertEqual(uncovered(report), set())
        dump["kit::ReadInput"]["limit"] = 30
        report = run(items, spec, contracts=contracts, rules=rules, mapping=entry, defaults=dump)
        self.assertEqual(len(report.uncovered), 1)
        self.assertEqual(len(report.dead), 1)


class DefaultValues(unittest.TestCase):
    """A defaulted Python parameter's value against the Rust default the
    defaults dump holds for the field it stands for."""

    DEFERRED = 'deferred_default = { python = "None", decision = "ref" }\n'
    DUMP = {"kit::ReadLimits": {"max_lines": 2000}, "kit::ReadInput": {"path": "a", "offset": 1}}

    @staticmethod
    def _limit(rust_type: str, python: str):
        """The read tool whose input has a `limit` field of `rust_type`, and
        Python's read taking `limit=<python>`."""
        items = read_tool()
        items[2]["fields"].append({"name": "limit", "type": rust_type})
        spec = {"pkg.ReadTool": dict(READ_SPEC["pkg.ReadTool"])}
        spec["pkg.ReadTool"]["read"] = ["async def", "path", "*", "offset=1", f"limit={python}"]
        contracts = json.loads(json.dumps(READ_CONTRACTS))
        contracts["types"]["ReadInput"]["fields"].append("limit")
        return items, spec, contracts

    def test_python_and_rust_with_the_same_defaults(self):
        report = run(
            read_tool(),
            READ_SPEC,
            contracts=READ_CONTRACTS,
            rules=self.DEFERRED,
            defaults=self.DUMP,
        )
        self.assertEqual(keys(report), set())

    def test_a_default_rust_does_not_share_fails(self):
        """Python says limit=20, Rust's default is 10."""
        items, spec, contracts = self._limit("usize", "20")
        dump = {**self.DUMP, "kit::ReadInput": {"path": "a", "offset": 1, "limit": 10}}
        report = run(items, spec, contracts=contracts, rules=self.DEFERRED, defaults=dump)
        self.assertEqual(
            keys(report),
            {"rust kit::ReadInput::limit (default) [pkg.ReadTool.read(limit=20), Rust 10]"},
        )
        (finding,) = report.findings
        self.assertEqual(finding.detail, "Python defaults it to 20; Rust to 10")
        self.assertFalse(report.passed(strict=False))
        dump["kit::ReadInput"]["limit"] = 20
        report = run(items, spec, contracts=contracts, rules=self.DEFERRED, defaults=dump)
        self.assertEqual(keys(report), set())

    def test_python_none_defers_to_a_rust_default_by_a_rule(self):
        """`[rules] deferred_default`: Python's None for a field that is no
        Option is Rust's default; without the rule, a finding."""
        without = run(read_tool(), READ_SPEC, contracts=READ_CONTRACTS, defaults=self.DUMP)
        self.assertEqual(
            keys(without),
            {
                "rust kit::ReadLimits::max_lines (default) "
                "[pkg.ReadTool.create(max_lines=None), Rust 2000]"
            },
        )
        items, spec, contracts = self._limit("Option<usize>", "None")
        dump = {**self.DUMP, "kit::ReadInput": {"path": "a", "offset": 1, "limit": 5}}
        report = run(items, spec, contracts=contracts, rules=self.DEFERRED, defaults=dump)
        self.assertEqual(
            keys(report),
            {"rust kit::ReadInput::limit (default) [pkg.ReadTool.read(limit=None), Rust 5]"},
        )

    def test_an_option_rust_resolves_at_execution_is_no_silent_pass(self):
        items, spec, contracts = self._limit("Option<usize>", "20")
        dump = {**self.DUMP, "kit::ReadInput": {"path": "a", "offset": 1, "limit": None}}
        report = run(items, spec, contracts=contracts, rules=self.DEFERRED, defaults=dump)
        (finding,) = report.findings
        self.assertEqual(
            finding.key,
            "rust kit::ReadInput::limit (default) [pkg.ReadTool.read(limit=20), Rust null]",
        )
        self.assertIn("resolves at execution", finding.detail)

    def test_a_struct_the_dump_lacks_fails_whatever_the_entries(self):
        dump = {"kit::ReadInput": self.DUMP["kit::ReadInput"]}
        entry = (
            '[[exception]]\napproved = 2026-11-02\nreason = "r"\ndecision = "ref"\n'
            'covers = ["rust kit::ReadLimits (dump)"]\n'
        )
        report = run(
            read_tool(),
            READ_SPEC,
            contracts=READ_CONTRACTS,
            rules=self.DEFERRED,
            mapping=entry,
            defaults=dump,
        )
        self.assertEqual(uncovered(report), {"rust kit::ReadLimits (dump)"})
        self.assertFalse(report.passed(strict=False))

    def test_a_builder_rust_does_not_call_is_pythons_none(self):
        items = [
            struct(
                "kit::FileDocs",
                methods=[
                    method("new", ("root", "&Path"), receiver=None, returns="Self"),
                    method("with_version", ("version", "String"), receiver="self", returns="Self"),
                ],
            )
        ]
        spec = {"pkg.FileDocs": {"__init__": ["def", "root", "*", "version=None"]}}
        self.assertEqual(keys(run(items, spec, defaults={})), set())
        spec["pkg.FileDocs"]["__init__"][-1] = "version='v1'"
        self.assertEqual(
            keys(run(items, spec, defaults={})),
            {"rust kit::FileDocs::with_version (default) [pkg.FileDocs.__init__(version='v1')]"},
        )

    def test_values_compare_as_json(self):
        """A tuple is a list; `True` is no 1."""
        items = [
            struct(
                "kit::Shell",
                methods=[
                    method(
                        "new",
                        ("root", "&Path"),
                        ("env", "ShellConfig"),
                        receiver=None,
                        returns="Self",
                    )
                ],
            ),
            struct(
                "kit::ShellConfig",
                fields=[("inherit", "Vec<String>"), ("strict", "bool")],
                impls=["Default"],
            ),
        ]
        spec = {"pkg.Shell": {"__init__": ["def", "root", "*", "inherit=('A',)", "strict=1"]}}
        dump = {"kit::ShellConfig": {"inherit": ["A"], "strict": True}}
        self.assertEqual(
            keys(run(items, spec, defaults=dump)),
            {"rust kit::ShellConfig::strict (default) [pkg.Shell.__init__(strict=1), Rust true]"},
        )


class Hidden(unittest.TestCase):
    """A public item `#[doc(hidden)]` hides is on no rustdoc page: each is a
    finding a `[[hidden]]` entry lists with its decision."""

    ITEMS = [function("kit::helper", ("value", "&str"))]
    SPECS = {"pkg.helper": {"__call__": ["def", "value"]}}
    LISTED = (
        '[[hidden]]\nreason = "plumbing"\ndecision = "ref"\n'
        'covers = ["rust kit::Tool::check (hidden)"]\n'
    )

    def test_a_hidden_item_is_listed_with_its_decision(self):
        report = run(self.ITEMS, self.SPECS, hidden=["kit::Tool::check"])
        self.assertEqual(uncovered(report), {"rust kit::Tool::check (hidden)"})
        report = run(self.ITEMS, self.SPECS, mapping=self.LISTED, hidden=["kit::Tool::check"])
        self.assertEqual(uncovered(report), set())
        self.assertEqual(len(report.hidden), 1)
        self.assertTrue(report.passed(strict=True))

    def test_an_item_no_longer_hidden_leaves_its_entry_dead(self):
        report = run(self.ITEMS, self.SPECS, mapping=self.LISTED, hidden=[])
        self.assertEqual(len(report.dead), 1)

    def test_only_a_hidden_entry_lists_a_hidden_item(self):
        for entry in (
            '[[pending]]\nreason = "r"\ncovers = ["rust kit::X (hidden)"]\n',
            '[[exception]]\napproved = 2026-11-02\nreason = "r"\ndecision = "ref"\n'
            'covers = ["rust kit::X (hidden)"]\n',
            '[[hidden]]\nreason = "r"\ndecision = "ref"\ncovers = ["rust kit::X (unmatched)"]\n',
            '[[hidden]]\nreason = "r"\ncovers = ["rust kit::X (hidden)"]\n',
        ):
            with self.subTest(entry=entry), self.assertRaises(ParityError):
                Mapping.from_table(tomllib.loads(mapping_text(entry)))

    def test_a_hidden_item_awaiting_its_decision_fails(self):
        entry = self.LISTED.replace('decision = "ref"', 'decision = "TODO: maintainer"')
        report = run(self.ITEMS, self.SPECS, mapping=entry, hidden=["kit::Tool::check"])
        self.assertEqual(uncovered(report), set())
        self.assertFalse(report.passed(strict=False))


class Decisions(unittest.TestCase):
    """Every rule and every approved entry names where the maintainer
    decided it; one whose decision is left to the maintainer fails the gate."""

    def test_a_rule_without_a_decision_is_refused(self):
        rules = tomllib.loads(mapping_text())["rules"]
        for name in rules:
            table = tomllib.loads(mapping_text())
            del table["rules"][name]["decision"]
            with self.subTest(rule=name), self.assertRaises(ParityError):
                Mapping.from_table(table)
        table = tomllib.loads(mapping_text())
        table["rules"]["factory"] = "create"
        with self.assertRaises(ParityError):
            Mapping.from_table(table)

    def test_each_base_rule_is_declared(self):
        """The rules the comparison applies with no setting are declared,
        each naming its decision: none is applied unwritten."""
        for name in ("receivers", "results", "asynchronous", "contract_types", "properties"):
            table = tomllib.loads(mapping_text())
            del table["rules"][name]
            with self.subTest(rule=name), self.assertRaises(ParityError):
                Mapping.from_table(table)

    def test_an_approved_entry_without_a_decision_is_refused(self):
        for entry in (
            '[[map]]\nrust = "kit::A"\npython = "pkg.B"\napproved = 2026-11-02\nreason = "r"\n',
            '[[exception]]\napproved = 2026-11-02\nreason = "r"\ncovers = ["rust kit::X (name)"]\n',
            '[[missing]]\napproved = 2026-11-02\nreason = "r"\n'
            'covers = ["rust kit::X (unmatched)"]\n',
        ):
            with self.subTest(entry=entry), self.assertRaises(ParityError):
                Mapping.from_table(tomllib.loads(mapping_text(entry)))
        pending = '[[pending]]\nreason = "r"\ncovers = ["rust kit::X (name)"]\n'
        self.assertEqual(len(Mapping.from_table(tomllib.loads(mapping_text(pending))).entries), 1)

    def test_a_decision_left_to_the_maintainer_fails_the_gate(self):
        rules = 'optional = { type = "Option", default = "None", decision = "TODO: maintainer" }'
        report = run(Entries.ITEMS, Entries.SPECS, contracts=Entries.CONTRACTS, rules=rules)
        self.assertEqual(report.undecided, ["[rules] optional"])
        self.assertFalse(report.passed(strict=False))
        missing = (
            '[[missing]]\napproved = 2026-11-02\nreason = "r"\ndecision = "TODO: maintainer"\n'
            'covers = ["rust kit::DocsListTool (unmatched)", "python pkg.DocsList (unmatched)"]\n'
        )
        report = run(Entries.ITEMS, Entries.SPECS, contracts=Entries.CONTRACTS, mapping=missing)
        self.assertEqual(uncovered(report), set())
        self.assertEqual(report.undecided, ["[[missing]] #1 (rust kit::DocsListTool (unmatched))"])
        self.assertFalse(report.passed(strict=False))


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
        (self.dir / "defaults.json").write_text("{}")
        (self.dir / "hidden.json").write_text(json.dumps({"hidden": {"kit": []}}))

    def tearDown(self):
        self._tmp.cleanup()

    def _main(self, mapping: str, *flags: str) -> tuple[int, str]:
        return self._run(mapping_text(mapping), *flags)

    def _run(self, text: str, *flags: str) -> tuple[int, str]:
        (self.dir / "mapping.toml").write_text(text)
        out = io.StringIO()
        status = gate.main(
            [
                str(self.dir / "surface.json"),
                "--python",
                str(self.dir / "python"),
                "--mapping",
                str(self.dir / "mapping.toml"),
                "--defaults",
                str(self.dir / "defaults.json"),
                "--hidden",
                str(self.dir / "hidden.json"),
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

    def test_a_decision_left_to_the_maintainer_fails(self):
        text = mapping_text(Entries.PAIRS).replace(
            'factory = { method = "create", decision = "ref" }',
            'factory = { method = "create", decision = "TODO: maintainer" }',
        )
        status, out = self._run(text)
        self.assertEqual(status, 1, out)
        self.assertIn("  [rules] factory", out)
        self.assertIn("1 awaiting the maintainer's decision", out)

    def test_an_input_it_cannot_read(self):
        self.assertEqual(self._main("[[nonsense]]\n")[0], 2)
        (self.dir / "hidden.json").write_text("[]")
        self.assertEqual(self._main(Entries.PAIRS)[0], 2)
        (self.dir / "defaults.json").write_text("[]")
        self.assertEqual(self._main(Entries.PAIRS)[0], 2)

    def test_the_hidden_items_are_required(self):
        (self.dir / "mapping.toml").write_text(mapping_text())
        argv = [str(self.dir / "surface.json"), "--defaults", str(self.dir / "defaults.json")]
        with self.assertRaises(SystemExit), contextlib.redirect_stderr(io.StringIO()):
            gate.main(argv, out=io.StringIO(), log=io.StringIO())


def _load_gate():
    spec = importlib.util.spec_from_file_location("api_parity_gate", HERE.parent / "api-parity.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


gate = _load_gate()


if __name__ == "__main__":
    unittest.main()
