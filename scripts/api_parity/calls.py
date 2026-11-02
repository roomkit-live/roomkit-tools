"""Python's `call` and `definition` against the typed Rust methods they stand for.

A typed method is a Rust method of `&self` taking an `<X>Input` and
returning an `Output<T>`: a tool call. Two rules read a Python member Rust
has no member of that name for:
- `[rules] typed_call`: Python's `call(arguments)` is the JSON form of the
  typed method's `<X>Input`: the class's one typed method, or the one Python
  also names among several;
- `[rules] named_calls`: Python's name-keyed `call(name, arguments, ...)`
  stands for the typed methods of the tool contracts its class serves, one
  method to one contract, by the contract's input type; the parameters after
  `name` are compared with each. Its static `definition(name)` stands for
  each such method's `<method>_definition()`. A contract with no typed
  method, or no definition, is a finding naming the contract; a typed method
  no contract of the class takes is left to the member comparison, which
  finds no Python member for it. `wire.call_types` checks each method's
  output against its contract's.
"""

from __future__ import annotations

from typing import TYPE_CHECKING

from api_parity.rust import base_type
from api_parity.sides import python_params
from api_parity.wire import output_of, tools_of

if TYPE_CHECKING:
    from api_parity.compare import Comparison, Member


def compare(
    comparison: Comparison,
    python: str,
    member: str,
    spec: dict,
    pool: dict,
    used: set,
    primary: str,
) -> bool:
    """Compare the class `spec`'s `member` by one of the rules above; False
    when none applies, the member then compared as any other."""
    if pool.get(member) or (python, member) in comparison.member_entries:
        return False
    rules, member_spec = comparison.rules, spec[member]
    params = python_params(member_spec)
    named = rules.setting("named_calls", "method")
    if member == named and params[:1] and params[0].name == rules.setting("named_calls", "key"):
        return _named_call(comparison, python, member, member_spec, pool, used, primary)
    if member == rules.setting("named_calls", "definition") and len(params) == 1:
        return _named_definitions(comparison, python, member, pool, used, primary)
    if member == rules.setting("typed_call", "method") and any(
        param.name == rules.arguments for param in params
    ):
        return _typed_call(comparison, python, member, spec, pool, used)
    return False


def typed(comparison: Comparison, pool: dict) -> list[Member]:
    """The pool's typed methods: tool calls taking an `<X>Input`."""
    return [
        found
        for members in pool.values()
        for found in members
        if found.kind == "method"
        and found.data["receiver"] is not None
        and output_of(found.data.get("returns") or "") is not None
        and inputs(comparison, found.data)
    ]


def inputs(comparison: Comparison, method: dict) -> list[str]:
    """The `<X>Input` types a method takes."""
    suffix = comparison.rules.input_suffix
    return [
        base_type(param["type"])
        for param in method["params"]
        if base_type(param["type"]).endswith(suffix)
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


def by_contract(comparison: Comparison, python: str, pool: dict) -> dict[str, Member | None]:
    """Each tool contract the class serves, by name, with the typed method
    taking its input type, or None."""
    methods = typed(comparison, pool)
    return {
        name: next((m for m in methods if tool["input"] in inputs(comparison, m.data)), None)
        for name, tool in sorted(tools_of(comparison, python).items())
    }


def _named_call(
    comparison: Comparison,
    python: str,
    member: str,
    spec: list,
    pool: dict,
    used: set,
    primary: str,
) -> bool:
    contracts = by_contract(comparison, python, pool)
    if not contracts:
        return False
    python_side = python_params(spec)[1:]
    for name, method in contracts.items():
        if method is None:
            wanted = tools_of(comparison, python)[name]["input"]
            detail = f"the tool {name} it serves: no Rust typed method takes {wanted}"
            where = f"{python}.{member}"
            comparison.find("python", where, "unmatched", detail, primary, python, qualifier=name)
            continue
        comparison.method(python, member, spec, method, used, python_side=python_side)
    return True


def _named_definitions(
    comparison: Comparison, python: str, member: str, pool: dict, used: set, primary: str
) -> bool:
    contracts = by_contract(comparison, python, pool)
    if not contracts:
        return False
    suffix = comparison.rules.setting("named_calls", "suffix") or ""
    for name, method in contracts.items():
        if method is None:
            continue  # the call's finding names the contract
        wanted = f"{method.data['name']}{suffix}"
        found = next(
            (
                m
                for m in pool.get(wanted, [])
                if m.kind == "method" and m.data["receiver"] is None and not m.data["params"]
            ),
            None,
        )
        if found is None:
            detail = f"the tool {name}'s definition: no Rust {wanted}()"
            where = f"{python}.{member}"
            comparison.find("python", where, "unmatched", detail, primary, python, qualifier=name)
        else:
            used.add((found.owner, wanted))
    return True
