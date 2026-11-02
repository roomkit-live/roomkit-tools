"""Each Python name against the Rust items standing for it.

The rules are written in `scripts/api-parity.py`'s docstring. This module
pairs the items and compares a class's members; `constructors`, `params`,
`errors`, `wire` and `leftovers` hold the other comparisons. A finding's kind
is one of `mapping.KINDS`.
"""

from __future__ import annotations

from dataclasses import dataclass

from api_parity import calls, constructors, errors, leftovers, wire
from api_parity import params as parameters
from api_parity.mapping import Entry, Mapping
from api_parity.rust import RustSide
from api_parity.sides import Python, Surface, python_params


@dataclass(frozen=True)
class Finding:
    side: str  # "rust" or "python": the side `path` is on
    path: str
    kind: str
    detail: str
    rust_item: str | None
    python_item: str | None

    @property
    def key(self) -> str:
        """What an entry's `covers` names it by."""
        return f"{self.side} {self.path} ({self.kind})"


@dataclass(frozen=True)
class Member:
    """A Rust member a Python one may stand for."""

    owner: str  # the Rust item, or the trait a method comes from
    data: dict
    inherited: bool
    field: bool


class Comparison:
    def __init__(self, surface: Surface, python: Python, mapping: Mapping) -> None:
        self.surface, self.python, self.rules = surface, python, mapping.rules
        self.rust = RustSide(surface, mapping.rules)
        self.findings: list[Finding] = []
        self.applied: set[int] = set()
        self.item_entries: dict[str, Entry] = {}
        self.member_entries: dict[tuple[str, str], Entry] = {}
        for entry in mapping.pairings():
            base, member, _ = surface.split(entry.rust)
            if member is None:
                self.item_entries[base] = entry
            else:
                owner, python_member, _ = python.split(entry.python)
                self.member_entries[(owner, python_member or "")] = entry
        self.roles: dict[str, str] = {}
        self.counterparts: dict[str, str] = {}  # a paired Rust item's Python name
        self.consumed: set[str] = set()  # the Python names compared
        self.builtin_errors_read: set[str] = set()
        self._keys: set[str] = set()

    def run(self) -> list[Finding]:
        groups = self._pair()
        for python, items in groups.items():
            self._group(python, items)
        wire.compare(self)
        leftovers.sweep(self)
        return self.findings

    def find(
        self, side: str, path: str, kind: str, detail: str, rust: str | None, python: str | None
    ) -> None:
        """Record a finding, once per key."""
        finding = Finding(side, path, kind, detail, rust, python)
        if finding.key not in self._keys:
            self._keys.add(finding.key)
            self.findings.append(finding)

    # Pairing: which Python name each Rust item stands for.

    def _pair(self) -> dict[str, list[dict]]:
        groups: dict[str, list[dict]] = {}
        for path, item in self.surface.items.items():
            python = self._counterpart(item)
            if python is not None:
                self.roles[path] = "paired"
                self.counterparts[path] = python
                groups.setdefault(python, []).append(item)
        return groups

    def _counterpart(self, item: dict) -> str | None:
        """The item's Python name, by an entry first, then by the rules; or
        the role that leaves it without one."""
        path = item["path"]
        name = path.rpartition("::")[2]
        entry = self.item_entries.get(path)
        if entry is not None and self.python.exists(entry.python):
            self.applied.add(id(entry))
            if entry.table != "map":
                short = entry.python.rpartition(".")[2]
                self.find("rust", path, "name", f"named {short} in Python", path, entry.python)
            return entry.python
        role = self._role(item)
        if role is not None:
            self.roles[path] = role
            return None
        modules = self.rules.modules_for(path)
        wanted = name if item["kind"] in ("fn", "constant") else self.rules.python_type_name(name)
        return self.python.find(modules, wanted)

    def _role(self, item: dict) -> str | None:
        if item["path"] in self.rules.builtins:
            return "builtin"
        if self.rust.is_future_alias(item):
            return "future"
        if self.rust.is_options(item):
            return "options"
        if self.rust.is_wire(item):
            return "wire"
        return None

    # A Python name against its Rust items.

    def _group(self, python: str, items: list[dict]) -> None:
        self.consumed.add(python)
        spec = self.python.specs.get(python)
        if spec is None:
            for item in items:
                if item["kind"] != "constant":
                    detail = f"a {item['kind']}; Python's {python} is a constant"
                    self.find("rust", item["path"], "form", detail, item["path"], python)
            return
        if "__call__" in spec:
            for item in items:
                self._function(python, spec["__call__"], item)
            return
        families = [
            item for item in items if item["kind"] == "enum" and "Error" in item.get("impls", [])
        ]
        for item in families:
            errors.compare(self, item, python)
        others = [item for item in items if item not in families]
        if others:
            self._class(python, spec, others)

    def _function(self, python: str, spec: list, item: dict) -> None:
        pairs = (item["path"], python)
        if item["kind"] != "fn":
            self.find(
                "rust",
                item["path"],
                "form",
                f"a {item['kind']}; Python's {python} is a function",
                *pairs,
            )
            return
        self.check_async(item["signature"], item["path"], spec, pairs)
        rust_side = self.rust.params(
            item["signature"], item["path"].rpartition("::")[0], arguments=False
        )
        parameters.compare(self, python, python_params(spec), rust_side, pairs, concrete=True)

    def _class(self, python: str, spec: dict, items: list[dict]) -> None:
        pool = self._pool(items)
        used: set[tuple[str, str]] = set()
        bases = [
            self.python.specs[base]
            for base in self.python.bases(python)
            if base in self.python.specs
        ]
        constructor = constructors.python_constructor(self, spec)
        interface = all(item["kind"] == "trait" for item in items)
        primary = items[0]["path"]
        for member, member_spec in spec.items():
            if member == "__bases__" or (
                member == "__init__" and constructor == self.rules.factory
            ):
                continue  # an async factory's __init__ wraps the native object it built
            if any(base.get(member) == member_spec for base in bases):
                continue  # compared on the base
            if member in self.rules.tool_protocol.members and not _static(member_spec):
                self._protocol_member(python, member, items)
            elif member == constructor:
                constructors.compare(self, python, member, member_spec, items, used)
            elif calls.compare(self, python, member, spec, pool, used):
                continue
            else:
                self._member(python, member, member_spec, pool, used, interface, primary)
        wire.call_types(self, python, items)
        for name, members in pool.items():
            for found in members:
                if (
                    found.inherited
                    or (found.owner, name) in used
                    or f"{found.owner}::{name}" in self.rust.consumed
                ):
                    continue
                what = "field" if found.field else "member"
                self.find(
                    "rust",
                    f"{found.owner}::{name}",
                    "unmatched",
                    f"no Python {what}",
                    primary,
                    python,
                )

    def _pool(self, items: list[dict]) -> dict[str, list[Member]]:
        """The Rust members of a Python class's items, by name: their own,
        and those of the surface's traits they implement."""
        pool: dict[str, list[Member]] = {}
        for item in items:
            own = set()
            for kind in ("methods", "required", "provided"):
                for method in item.get(kind, []):
                    own.add(method["name"])
                    pool.setdefault(method["name"], []).append(
                        Member(item["path"], method, False, False)
                    )
            for field in item.get("fields", []):
                if not field["name"].isdigit():
                    own.add(field["name"])
                    pool.setdefault(field["name"], []).append(
                        Member(item["path"], field, False, True)
                    )
            for name, (trait, method) in self.rust.trait_members(item).items():
                if name not in own:
                    pool.setdefault(name, []).append(Member(trait, method, True, False))
        return pool

    def _protocol_member(self, python: str, member: str, items: list[dict]) -> None:
        trait = self.rules.tool_protocol.trait
        if not any(trait in item.get("impls", []) for item in items):
            detail = f"the RoomKit tool protocol: no Rust item standing for it implements {trait}"
            self.find("python", f"{python}.{member}", "unmatched", detail, items[0]["path"], python)

    def _member(
        self,
        python: str,
        member: str,
        spec: list,
        pool: dict,
        used: set,
        interface: bool,
        primary: str,
    ) -> None:
        where = f"{python}.{member}"
        entry = self.member_entries.get((python, member))
        if entry is not None and self.rust.member(entry.rust) is not None:
            item, data = self.rust.member(entry.rust)
            self.paired_member(entry, python, item["path"])
            candidates = [Member(item["path"], data, False, "params" not in data)]
        else:
            candidates = pool.get(member, [])
        if spec[0] == "property":
            self._property(where, candidates, used, python, primary)
            return
        methods = [found for found in candidates if not found.field]
        if not methods:
            self.find("python", where, "unmatched", "no Rust method", primary, python)
            return
        static = _static(spec)
        found = next((m for m in methods if (m.data["receiver"] is None) == static), methods[0])
        renames = entry.params if entry is not None and entry.table == "map" else {}
        self.method(python, member, spec, found, used, renames=renames, concrete=not interface)

    def method(
        self,
        python: str,
        member: str,
        spec: list,
        found: Member,
        used: set,
        *,
        renames: dict[str, str] | None = None,
        concrete: bool = True,
        python_side: list | None = None,
    ) -> None:
        """A Python method against the Rust one standing for it: its form,
        async, and parameters (`python_side`, or the spec's)."""
        used.add((found.owner, found.data["name"]))
        rust_path = f"{found.owner}::{found.data['name']}"
        pairs = (found.owner, python)
        static = _static(spec)
        if (found.data["receiver"] is None) != static:
            rust_form = (
                "an associated function" if found.data["receiver"] is None else "a method of self"
            )
            self.find(
                "rust",
                rust_path,
                "form",
                f"{rust_form}; Python's {member} is a {spec[0].split()[-1]}",
                *pairs,
            )
        self.check_async(found.data, found.owner, spec, pairs, where=rust_path)
        if python_side is None:
            python_side = python_params(spec)
        arguments = any(param.name == self.rules.arguments for param in python_side)
        rust_side = self.rust.params(found.data, found.owner, arguments=arguments)
        rust_side = parameters.renamed(rust_side, renames or {})
        where = f"{python}.{member}"
        parameters.compare(self, where, python_side, rust_side, pairs, concrete=concrete)

    def _property(
        self, where: str, candidates: list[Member], used: set, python: str, primary: str
    ) -> None:
        """A property is a field, or a method taking `&self` alone."""
        if not candidates:
            self.find(
                "python", where, "unmatched", "a property: no Rust method or field", primary, python
            )
            return
        found = next((c for c in candidates if c.field), candidates[0])
        used.add((found.owner, found.data["name"]))
        if found.field:
            return
        data = found.data
        if data["params"] or data["receiver"] is None:
            detail = f"{where} is a property: Rust's is no method of &self alone"
            self.find("rust", f"{found.owner}::{data['name']}", "form", detail, found.owner, python)

    def paired_member(self, entry: Entry, python: str, rust_item: str) -> None:
        """A member an entry pairs: a name finding, unless the entry maps."""
        self.applied.add(id(entry))
        if entry.table == "map":
            return
        member = entry.python.rpartition(".")[2]
        self.find("rust", entry.rust, "name", f"named {member} in Python", rust_item, python)

    def check_async(
        self, method: dict, owner: str, spec: list, pairs: tuple, where: str | None = None
    ) -> None:
        python_async = spec[0].startswith("async")
        rust_async = self.rust.is_async(method, owner)
        if python_async != rust_async:
            detail = (
                "async in Python, not in Rust" if python_async else "async in Rust, not in Python"
            )
            self.find("rust", where or owner, "async", detail, *pairs)

    def non_exhaustive(self, item: dict, python: str | None) -> None:
        """A struct a caller builds keeps no `#[non_exhaustive]`: outside its
        crate that refuses the struct update standing for Python's keywords
        (roomkit-rs convention 9)."""
        detail = "#[non_exhaustive]: a caller outside the crate cannot build it by struct update"
        self.find("rust", item["path"], "non_exhaustive", detail, item["path"], python)


def _static(spec: list) -> bool:
    """A static or class method: Rust's associated function."""
    return spec[0].split()[-1] in ("staticmethod", "classmethod")
