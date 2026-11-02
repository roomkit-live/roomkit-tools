"""A Python constructor against the Rust ones.

Python's constructor is an async factory (`[rules] factory`) when the class
has one taking more than `**keywords`, Python being unable to await in
`__init__`; else `__init__`. Rust's is `new` and the builders, a model's
fields, or the Rust method an entry pairs with it.
"""

from __future__ import annotations

import re
from typing import TYPE_CHECKING

from api_parity import params as parameters
from api_parity.rust import base_type
from api_parity.sides import PythonParam, python_params

if TYPE_CHECKING:
    from api_parity.compare import Comparison


def python_constructor(comparison: Comparison, spec: dict) -> str | None:
    factory = spec.get(comparison.rules.factory)
    if (
        factory
        and factory[0] == "async classmethod"
        and not all(t.startswith("**") for t in factory[1:])
    ):
        return comparison.rules.factory
    return "__init__" if "__init__" in spec else None


def compare(
    comparison: Comparison, python: str, member: str, spec: list, items: list[dict], used: set
) -> None:
    where = f"{python}.{member}"
    for item, method in _targets(comparison, python, member, spec, items, used):
        pairs = (item["path"], python)
        if method is None:
            rust_side = comparison.rust.fields(item, group=None, builder=False)
            used.update((item["path"], field["name"]) for field in item.get("fields", []))
            if item.get("non_exhaustive"):
                comparison.non_exhaustive(item, python)
        else:
            rust_path = f"{item['path']}::{method['name']}"
            comparison.check_async(method, item["path"], spec, pairs, where=rust_path)
            _returns(comparison, item, method, pairs)
            if _wraps(comparison, item, method, spec, pairs):
                continue
            rust_side = comparison.rust.constructor(item, method)
        parameters.compare(comparison, where, python_params(spec), rust_side, pairs, concrete=True)


def _targets(
    comparison: Comparison, python: str, member: str, spec: list, items: list[dict], used: set
) -> list[tuple[dict, dict | None]]:
    """Each Rust constructor to compare: the method an entry pairs, else
    each item's `new`, or a model's fields (`None`)."""
    entry = comparison.member_entries.get((python, member))
    found = comparison.rust.member(entry.rust) if entry is not None else None
    if found is not None:
        item, method = found
        comparison.paired_member(entry, python, items[0]["path"])
        used.add((item["path"], method["name"]))
        return [(item, method)]
    targets: list[tuple[dict, dict | None]] = []
    for item in items:
        if item["kind"] == "trait":
            continue
        new = comparison.rust.method(item, "new", receiver=False)
        if new is not None:
            targets.append((item, new))
            used.add((item["path"], "new"))
        elif item["kind"] == "struct" and item.get("fields"):
            targets.append((item, None))
        elif python_params(spec):
            detail = f"no Rust constructor (new) on {item['path']}"
            comparison.find(
                "python", f"{python}.{member}", "unmatched", detail, item["path"], python
            )
    return targets


def _returns(comparison: Comparison, item: dict, method: dict, pairs: tuple) -> None:
    """A constructor builds its own type."""
    name = item["path"].rpartition("::")[2]
    returns = method.get("returns") or ""
    if not re.search(rf"\b(?:Self|{re.escape(name)})\b", returns):
        detail = f"returns {returns or 'nothing'}, not a {name}"
        comparison.find("rust", f"{item['path']}::{method['name']}", "result", detail, *pairs)


def _wraps(comparison: Comparison, item: dict, method: dict, spec: list, pairs: tuple) -> bool:
    """A Rust constructor taking a built tool, where Python takes that tool's
    own constructor parameters: one finding, not one per parameter."""
    params = [param for param in method["params"] if not comparison.rust.is_cancellation(param)]
    if len(params) != 1 or comparison.rust.builders(item):
        return False
    wrapped = comparison.surface.named("struct", base_type(params[0]["type"]), item["path"])
    python = comparison.counterparts.get(wrapped["path"]) if wrapped else None
    wrapped_spec = comparison.python.specs.get(python or "", {})
    constructor = python_constructor(comparison, wrapped_spec)
    if constructor is None:
        return False
    ours, theirs = python_params(spec), python_params(wrapped_spec[constructor])
    if not (_covered(ours, theirs) and _covered(theirs, ours)):
        return False
    names = ", ".join(("**" if param.variadic else "") + param.name for param in ours)
    built = wrapped["path"].rpartition("::")[2]
    if python == pairs[1]:
        how = f"Python's {pairs[1]} is that tool itself, built from ({names})"
    else:
        how = f"Python's {pairs[1]} takes {python}.{constructor}'s parameters ({names})"
    rust_path = f"{item['path']}::{method['name']}({params[0]['name']})"
    comparison.find("rust", rust_path, "form", f"takes a built {built}; {how}", *pairs)
    return True


def _covered(these: list[PythonParam], those: list[PythonParam]) -> bool:
    """Whether each named parameter of `these` is one of `those`, or
    `those` takes `**keywords`."""
    names = {param.name for param in those if not param.variadic}
    return any(param.variadic == "**" for param in those) or all(
        param.name in names for param in these if not param.variadic
    )
