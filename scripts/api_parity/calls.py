"""Python's `call` and `definition` against the typed Rust methods they stand for.

A typed method is a Rust method of `&self` taking an `<X>Input` and
returning an `Output<T>`: a tool call. Two rules read a Python member Rust
has no member of that name for:
- `[rules] typed_call`: Python's `call(arguments)` is the JSON form of the
  typed method's `<X>Input`: the class's one typed method, or the one Python
  also names among several;
- `[rules] named_calls`: Python's name-keyed `call(name, arguments, ...)`
  stands for every typed method, the name selecting one, the parameters
  after `name` compared with each; its static `definition(name)` stands for
  every associated function `<x>_definition()`.
"""

from __future__ import annotations

from typing import TYPE_CHECKING

from api_parity.rust import base_type
from api_parity.sides import python_params
from api_parity.wire import output_of

if TYPE_CHECKING:
    from api_parity.compare import Comparison, Member


def compare(
    comparison: Comparison, python: str, member: str, spec: dict, pool: dict, used: set
) -> bool:
    """Compare the class `spec`'s `member` by one of the rules above; False
    when none applies, the member then compared as any other."""
    if pool.get(member) or (python, member) in comparison.member_entries:
        return False
    rules, member_spec = comparison.rules, spec[member]
    params = python_params(member_spec)
    named = rules.setting("named_calls", "method")
    if member == named and params[:1] and params[0].name == rules.setting("named_calls", "key"):
        return _named_call(comparison, python, member, member_spec, pool, used)
    if member == rules.setting("named_calls", "definition") and len(params) == 1:
        return _named_definitions(comparison, pool, used)
    if member == rules.setting("typed_call", "method") and any(
        param.name == rules.arguments for param in params
    ):
        return _typed_call(comparison, python, member, spec, pool, used)
    return False


def typed(comparison: Comparison, pool: dict) -> list[Member]:
    """The pool's typed methods: tool calls taking an `<X>Input`."""
    suffix = comparison.rules.input_suffix
    return [
        found
        for members in pool.values()
        for found in members
        if not found.field
        and found.data["receiver"] is not None
        and output_of(found.data.get("returns") or "") is not None
        and any(base_type(param["type"]).endswith(suffix) for param in found.data["params"])
    ]


def _typed_call(
    comparison: Comparison, python: str, member: str, spec: dict, pool: dict, used: set
) -> bool:
    found = typed(comparison, pool)
    if len(found) > 1:
        found = [method for method in found if method.data["name"] in spec]
    if len(found) != 1:
        return False
    comparison.method(python, member, spec[member], found[0], used)
    return True


def _named_call(
    comparison: Comparison, python: str, member: str, spec: list, pool: dict, used: set
) -> bool:
    found = typed(comparison, pool)
    if not found:
        return False
    python_side = python_params(spec)[1:]
    for method in found:
        comparison.method(python, member, spec, method, used, python_side=python_side)
    return True


def _named_definitions(comparison: Comparison, pool: dict, used: set) -> bool:
    suffix = comparison.rules.setting("named_calls", "suffix") or ""
    found = [
        method
        for name, members in pool.items()
        if name.endswith(suffix)
        for method in members
        if not method.field and method.data["receiver"] is None and not method.data["params"]
    ]
    used.update((method.owner, method.data["name"]) for method in found)
    return bool(found)
