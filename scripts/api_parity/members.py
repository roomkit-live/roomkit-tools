"""A Python class's members against those of the Rust items standing for it.

The pool of a class's Rust members holds the items' own members and the
methods of the surface's traits they implement. Each Python member is
compared with the pool's member of its name, or the one an entry pairs; a
member a Python base repeats unchanged is compared on that base when the
base stands for a trait the Rust items implement. An async factory's
`__init__` is compared as a constructor too, unless it takes only the native
object the factory built, which the snapshot marks `__native__`. What the
comparison left in the pool is a finding: an own member with no Python one, and a trait
member the Python class lacks, unless a Python base stands for that trait
(the trait's own comparison then reports it).
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import TYPE_CHECKING

from api_parity import calls, constructors, values, wire
from api_parity.sides import MARKERS, native_params, python_params

if TYPE_CHECKING:
    from api_parity.compare import Comparison


@dataclass(frozen=True)
class Member:
    """A Rust member a Python one may stand for."""

    owner: str  # the Rust item, or the trait a method comes from
    data: dict
    inherited: bool
    kind: str  # "method", "field", "constant" or "variant"
    implementor: str | None = None  # the item implementing the trait, if inherited

    @property
    def field(self) -> bool:
        return self.kind == "field"


def compare(comparison: Comparison, python: str, spec: dict, items: list[dict]) -> None:
    """The class `python`'s members against those of `items`."""
    pool = _pool(comparison, items)
    used: set[tuple[str, str]] = set()
    constructor = constructors.python_constructor(comparison, spec)
    interface = all(item["kind"] == "trait" for item in items)
    primary = items[0]["path"]
    trait_bases = _trait_bases(comparison, python, items)
    rules = comparison.rules
    for member, member_spec in spec.items():
        if member in MARKERS:
            continue
        if member == rules.factory and constructors.uniform_factory(comparison, spec):
            continue  # Python's uniform async constructor; __init__ is compared
        if member == "__init__" and constructor == rules.factory:
            if not _wraps_native(comparison, spec):
                constructors.compare(comparison, python, member, member_spec, items, used)
            continue
        if any(comparison.python.specs[base].get(member) == member_spec for base in trait_bases):
            continue  # compared on the base, which stands for a trait the items implement
        if member_spec[0] == "member":
            _constant(comparison, python, member, member_spec, pool, used, primary)
        elif member in rules.tool_protocol.members and not static(member_spec):
            _protocol_member(comparison, python, member, items)
        elif member == constructor:
            constructors.compare(comparison, python, member, member_spec, items, used)
        elif calls.compare(comparison, python, member, spec, pool, used, primary):
            continue
        else:
            _member(comparison, python, member, member_spec, pool, used, interface, primary)
    wire.call_types(comparison, python, items)
    _leftovers(comparison, python, pool, used, primary, trait_bases)


def _wraps_native(comparison: Comparison, spec: dict) -> bool:
    """An async factory's `__init__` wrapping the native object the factory
    built, a parameter annotated with private native types (`__native__`):
    not compared when it takes that object alone, nor, by `[rules]
    native_wrapper`, when that object is its first parameter, whatever else
    it takes."""
    params = python_params(spec["__init__"])
    if not params or params[0].variadic or params[0].name not in native_params(spec):
        return False
    wrapper = comparison.rules.setting("native_wrapper", "python") == "__init__"
    return len(params) == 1 or wrapper


def _trait_bases(comparison: Comparison, python: str, items: list[dict]) -> list[str]:
    """The Python bases of `python` standing for a trait the items implement."""
    implemented = {
        trait for item in items for trait, _ in comparison.rust.trait_members(item).values()
    }
    standing = {comparison.counterparts.get(trait) for trait in implemented}
    return [
        base
        for base in comparison.python.bases(python)
        if base in standing and base in comparison.python.specs
    ]


def _pool(comparison: Comparison, items: list[dict]) -> dict[str, list[Member]]:
    """The Rust members of a Python class's items, by name: their own,
    and those of the surface's traits they implement."""
    pool: dict[str, list[Member]] = {}
    for item in items:
        own = set()
        for kind in ("methods", "required", "provided"):
            for method in item.get(kind, []):
                own.add(method["name"])
                pool.setdefault(method["name"], []).append(
                    Member(item["path"], method, False, "method")
                )
        for field in item.get("fields", []):
            if not field["name"].isdigit():
                own.add(field["name"])
                pool.setdefault(field["name"], []).append(
                    Member(item["path"], field, False, "field")
                )
        for kind, found in (("constant", item.get("constants", [])), ("variant", _variants(item))):
            for member in found:
                own.add(member["name"])
                pool.setdefault(member["name"], []).append(
                    Member(item["path"], member, False, kind)
                )
        for name, (trait, method) in comparison.rust.trait_members(item).items():
            if name not in own:
                pool.setdefault(name, []).append(
                    Member(trait, method, True, "method", item["path"])
                )
    return pool


def _leftovers(
    comparison: Comparison,
    python: str,
    pool: dict[str, list[Member]],
    used: set,
    primary: str,
    trait_bases: list[str],
) -> None:
    """The pool's members no Python member stood for."""
    for name, members in pool.items():
        for found in members:
            if (found.owner, name) in used or f"{found.owner}::{name}" in comparison.rust.consumed:
                continue
            if not found.inherited:
                what = {"field": "field", "constant": "constant"}.get(found.kind, "member")
                path = f"{found.owner}::{name}"
                comparison.find("rust", path, "unmatched", f"no Python {what}", primary, python)
            elif comparison.counterparts.get(found.owner) not in trait_bases:
                trait = found.owner.rpartition("::")[2]
                detail = (
                    f"implements {trait}'s {name}: no member of {python}, "
                    f"nor of a Python base standing for {trait}"
                )
                path = f"{found.implementor}::{name}"
                comparison.find("rust", path, "unmatched", detail, found.implementor, python)


def _variants(item: dict) -> list[dict]:
    """An enum's variants: one standing for a Python class, neither a
    contract type nor an error, is a Python enum of the same members."""
    return item.get("variants", []) if item["kind"] == "enum" else []


def _constant(
    comparison: Comparison,
    python: str,
    member: str,
    spec: list,
    pool: dict,
    used: set,
    primary: str,
) -> None:
    """A Python enum member or class constant: a Rust variant, or an
    associated constant of the same value."""
    where = f"{python}.{member}"
    found = next((m for m in pool.get(member, []) if m.kind in ("constant", "variant")), None)
    if found is None:
        detail = "no Rust associated constant or variant"
        comparison.find("python", where, "unmatched", detail, primary, python)
        return
    used.add((found.owner, member))
    if found.kind == "variant":
        return
    rust = found.data.get("value")
    if not values.same_literal(spec[1] if len(spec) > 1 else None, rust):
        detail = f"Python's value is {spec[1:2]}; Rust's {rust or 'is not shown'}"
        qualifier = f"{where}={spec[1] if len(spec) > 1 else ''}, Rust {rust}"
        path = f"{found.owner}::{member}"
        comparison.find("rust", path, "value", detail, found.owner, python, qualifier=qualifier)


def _protocol_member(comparison: Comparison, python: str, member: str, items: list[dict]) -> None:
    trait = comparison.rules.tool_protocol.trait
    if not any(trait in item.get("impls", []) for item in items):
        detail = f"the RoomKit tool protocol: no Rust item standing for it implements {trait}"
        where = f"{python}.{member}"
        comparison.find("python", where, "unmatched", detail, items[0]["path"], python)


def _member(
    comparison: Comparison,
    python: str,
    member: str,
    spec: list,
    pool: dict,
    used: set,
    interface: bool,
    primary: str,
) -> None:
    where = f"{python}.{member}"
    entry = comparison.member_entries.get((python, member))
    if entry is not None and comparison.rust.member(entry.rust) is not None:
        item, data = comparison.rust.member(entry.rust)
        comparison.paired_member(entry, python, item["path"])
        kind = "method" if "params" in data else "field"
        candidates = [Member(item["path"], data, False, kind)]
    else:
        candidates = pool.get(member, [])
    if spec[0] == "property":
        _property(comparison, where, candidates, used, python, primary)
        return
    methods = [found for found in candidates if found.kind == "method"]
    if not methods:
        comparison.find("python", where, "unmatched", "no Rust method", primary, python)
        return
    wanted = static(spec)
    found = next((m for m in methods if (m.data["receiver"] is None) == wanted), methods[0])
    renames = entry.params if entry is not None and entry.table == "map" else {}
    comparison.method(python, member, spec, found, used, renames=renames, concrete=not interface)


def _property(
    comparison: Comparison,
    where: str,
    candidates: list[Member],
    used: set,
    python: str,
    primary: str,
) -> None:
    """A property is a field, or a method taking `&self` alone."""
    if not candidates:
        detail = "a property: no Rust method or field"
        comparison.find("python", where, "unmatched", detail, primary, python)
        return
    found = next((c for c in candidates if c.field), candidates[0])
    used.add((found.owner, found.data["name"]))
    if found.field:
        return
    data = found.data
    if data["params"] or data["receiver"] is None:
        detail = f"{where} is a property: Rust's is no method of &self alone"
        path = f"{found.owner}::{data['name']}"
        comparison.find("rust", path, "form", detail, found.owner, python, qualifier=where)


def static(spec: list) -> bool:
    """A static or class method: Rust's associated function."""
    return spec[0].split()[-1] in ("staticmethod", "classmethod")
