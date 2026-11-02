"""The contract types against the JSON the Python calls exchange.

A Rust type deriving `JsonSchema` is the JSON a Python call passes or
returns as a dict, so it has no Python class. Its public fields are the
properties of the JSON type of its name in the tool contracts the Python
snapshot holds; a unit variant is a value (its `snake_case` name, or, for a
value that is no identifier, the value at its position); a variant with
fields is a tagged variant, its fields that variant's properties. The JSON
side of a finding is written `json:<Type>.<property>`.
"""

from __future__ import annotations

import re
from typing import TYPE_CHECKING

from api_parity.mapping import snake_name
from api_parity.rust import base_type, python_name

if TYPE_CHECKING:
    from api_parity.compare import Comparison

_IDENTIFIER = re.compile(r"[a-z_][a-z0-9_]*")


def compare(comparison: Comparison) -> None:
    for path, role in comparison.roles.items():
        if role == "wire":
            _wire(comparison, comparison.surface.items[path])


def _wire(comparison: Comparison, item: dict) -> None:
    path = item["path"]
    name = path.rpartition("::")[2]
    _meaning(comparison, item, name)
    _own_members(comparison, item)
    if (
        comparison.rust.is_input(item)
        and path in comparison.rust.expanded
        and item.get("non_exhaustive")
    ):
        comparison.non_exhaustive(item, None)
    shape = comparison.python.contracts["types"].get(name)
    if shape is None:
        detail = "a contract type no tool contract holds: its JSON is compared with nothing"
        comparison.find("rust", path, "uncompared", detail, path, None)
        return
    if item["kind"] == "struct":
        _fields(comparison, item, shape, name)
    elif any(
        variant.get("fields") or "(" in variant["decl"] for variant in item.get("variants", [])
    ):
        _tagged(comparison, item, shape, name)
    else:
        _values(comparison, item, shape, name)


def _meaning(comparison: Comparison, item: dict, name: str) -> None:
    """A contract type named as a Python class that is something else."""
    python = comparison.python.find(comparison.rules.modules_for(item["path"]), name)
    if python is not None and python in comparison.python.specs:
        detail = f"a contract type; Python's {python} of the same name is another thing, a class"
        comparison.find("rust", item["path"], "meaning", detail, item["path"], python)


def _own_members(comparison: Comparison, item: dict) -> None:
    for method in item.get("methods", []):
        member = f"{item['path']}::{method['name']}"
        if member not in comparison.rust.consumed:
            detail = "a contract type's own member: no Python counterpart"
            comparison.find("rust", member, "unmatched", detail, item["path"], None)


def _fields(comparison: Comparison, item: dict, shape: dict, name: str) -> None:
    if "fields" not in shape:
        comparison.find(
            "rust",
            item["path"],
            "form",
            f"a struct; JSON's {name} is {_what(shape)}",
            item["path"],
            f"json:{name}",
        )
        return
    rust = {
        python_name(field["name"]): field["name"]
        for field in item.get("fields", [])
        if not field["name"].isdigit()
    }
    _both(comparison, item["path"], name, rust, shape["fields"], "field", "property")


def _values(comparison: Comparison, item: dict, shape: dict, name: str) -> None:
    if "values" not in shape:
        comparison.find(
            "rust",
            item["path"],
            "form",
            f"an enum of values; JSON's {name} is {_what(shape)}",
            item["path"],
            f"json:{name}",
        )
        return
    values = shape["values"]
    variants = [variant["name"] for variant in item.get("variants", [])]
    rust: dict[str, str] = {}
    for position, variant in enumerate(variants):
        value = snake_name(variant)
        if (
            value not in values
            and position < len(values)
            and not _IDENTIFIER.fullmatch(str(values[position]))
        ):
            value = str(values[position])
        rust[value] = variant
    _both(
        comparison, item["path"], name, rust, [str(value) for value in values], "variant", "value"
    )


def _tagged(comparison: Comparison, item: dict, shape: dict, name: str) -> None:
    if "variants" not in shape:
        comparison.find(
            "rust",
            item["path"],
            "form",
            f"an enum with fields; JSON's {name} is {_what(shape)}",
            item["path"],
            f"json:{name}",
        )
        return
    variants = {snake_name(variant["name"]): variant for variant in item.get("variants", [])}
    _both(
        comparison,
        item["path"],
        name,
        {tag: v["name"] for tag, v in variants.items()},
        list(shape["variants"]),
        "variant",
        "variant",
    )
    for tag, variant in variants.items():
        properties = shape["variants"].get(tag)
        if properties is None:
            continue
        rust = {python_name(field["name"]): field["name"] for field in variant.get("fields", [])}
        _both(
            comparison,
            f"{item['path']}::{variant['name']}",
            f"{name}.{tag}",
            rust,
            properties,
            "field",
            "property",
        )


def _both(
    comparison: Comparison,
    rust_path: str,
    json_path: str,
    rust: dict[str, str],
    json: list[str],
    member: str,
    prop: str,
) -> None:
    """Each Rust member a JSON one of the same name, and back."""
    owner = rust_path.split("::")
    item = "::".join(owner[:2])
    for name, rust_name in rust.items():
        if name not in json:
            comparison.find(
                "rust",
                f"{rust_path}::{rust_name}",
                "unmatched",
                f"a {member} with no JSON {prop} in {json_path}",
                item,
                f"json:{json_path}",
            )
    for name in json:
        if name not in rust:
            comparison.find(
                "python",
                f"json:{json_path}.{name}",
                "unmatched",
                f"a JSON {prop}: no Rust {member} of {rust_path}",
                item,
                f"json:{json_path}",
            )


def _what(shape: dict) -> str:
    if "fields" in shape:
        return "an object"
    if "values" in shape:
        return "an enumeration of values"
    if "variants" in shape:
        return "a tagged union"
    return "an array"


def tools_of(comparison: Comparison, python: str) -> dict[str, dict]:
    """The tool contracts a Python class serves, by name."""
    return {
        name: tool
        for name, tool in comparison.python.contracts["tools"].items()
        if tool.get("class") == python
    }


def call_types(comparison: Comparison, python: str, items: list[dict]) -> None:
    """The types of a class's tool calls. Each Rust method returning an
    `Output<T>` serves the contract of the class taking its input type, and
    returns that contract's output type; a class serving one contract with
    one such method compares its input with the contract's too."""
    tools = tools_of(comparison, python)
    calls = [
        (item, method)
        for item in items
        for method in item.get("methods", [])
        if output_of(method.get("returns") or "") is not None
    ]
    if not tools or not calls:
        return
    single = len(tools) == 1 and len(calls) == 1
    for item, method in calls:
        taken = _inputs(comparison, method)
        name = next(
            (name for name, tool in sorted(tools.items()) if tool["input"] in taken),
            next(iter(tools)) if single else None,
        )
        where = f"{item['path']}::{method['name']}"
        if name is None or taken != [tools[name]["input"]]:
            _foreign_input(comparison, where, item, python, taken, tools, name)
            continue
        returned = output_of(method["returns"])
        if returned != tools[name]["output"]:
            detail = f"returns Output<{returned}>; the contract's output is {tools[name]['output']}"
            qualifier = f"{name}: {returned}, contract {tools[name]['output']}"
            comparison.find(
                "rust", where, "result", detail, item["path"], python, qualifier=qualifier
            )


def _inputs(comparison: Comparison, method: dict) -> list[str]:
    suffix = comparison.rules.input_suffix
    return [base_type(p["type"]) for p in method["params"] if base_type(p["type"]).endswith(suffix)]


def _foreign_input(
    comparison: Comparison,
    where: str,
    item: dict,
    python: str,
    taken: list[str],
    tools: dict[str, dict],
    name: str | None,
) -> None:
    """A call taking another input than its contract's, or than any
    contract of its class."""
    shown = ", ".join(taken) or "no input struct"
    if name is not None:
        detail = f"takes {shown}; the contract's input is {tools[name]['input']}"
        qualifier = f"{name}: {shown}, contract {tools[name]['input']}"
    else:
        detail = f"takes {shown}; no tool contract of {python} takes it"
        qualifier = shown
    comparison.find("rust", where, "result", detail, item["path"], python, qualifier=qualifier)


def output_of(returns: str) -> str | None:
    """The `T` of a return type holding an `Output<T>`, by its name; a
    `Vec<T>` is the contracts' `Array_of_T`."""
    found = re.search(r"\bOutput<\s*(?:Vec<\s*([A-Za-z_][\w:]*)|([A-Za-z_][\w:]*))", returns)
    if found is None:
        return None
    if found.group(1):
        return "Array_of_" + found.group(1).rpartition("::")[2]
    return found.group(2).rpartition("::")[2]
