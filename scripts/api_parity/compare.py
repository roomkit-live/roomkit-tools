"""Each Python name against the Rust items standing for it.

The rules are written in `scripts/api-parity.py`'s docstring. This module
pairs the items and compares a function or a method; `members` compares a
class's members, and `constructors`, `params`, `errors`, `wire` and
`leftovers` hold the other comparisons. A finding's kind is one of
`mapping.KINDS`.
"""

from __future__ import annotations

from dataclasses import dataclass

from api_parity import errors, leftovers, members, wire
from api_parity import params as parameters
from api_parity.defaults import Defaults
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
    # What the finding was compared with, when the path alone does not say:
    # the other side's call or name, the fields a struct stands for, both
    # default values. An entry covering it is dead once that changes.
    qualifier: str | None = None

    @property
    def key(self) -> str:
        """What an entry's `covers` names it by."""
        key = f"{self.side} {self.path} ({self.kind})"
        return f"{key} [{self.qualifier}]" if self.qualifier else key


class Comparison:
    def __init__(
        self, surface: Surface, python: Python, mapping: Mapping, defaults: Defaults | None = None
    ) -> None:
        self.surface, self.python, self.rules = surface, python, mapping.rules
        # The Rust defaults dump; None compares no default value.
        self.defaults = defaults
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
        self,
        side: str,
        path: str,
        kind: str,
        detail: str,
        rust: str | None,
        python: str | None,
        *,
        qualifier: str | None = None,
    ) -> None:
        """Record a finding, once per key."""
        finding = Finding(side, path, kind, detail, rust, python, qualifier)
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
                detail = f"named {short} in Python"
                self.find("rust", path, "name", detail, path, entry.python, qualifier=entry.python)
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
            members.compare(self, python, spec, others)

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
        self.check_async(item["signature"], item["path"], spec, pairs, item["path"], python)
        rust_side = self.rust.params(
            item["signature"], item["path"].rpartition("::")[0], arguments=False
        )
        sides = parameters.Sides(python, item["path"], pairs)
        parameters.compare(self, sides, python_params(spec), rust_side, concrete=True)

    def method(
        self,
        python: str,
        member: str,
        spec: list,
        found: members.Member,
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
        where = f"{python}.{member}"
        if (found.data["receiver"] is None) != members.static(spec):
            rust_form = (
                "an associated function" if found.data["receiver"] is None else "a method of self"
            )
            detail = f"{rust_form}; Python's {member} is a {spec[0].split()[-1]}"
            self.find("rust", rust_path, "form", detail, *pairs, qualifier=where)
        self.check_async(found.data, found.owner, spec, pairs, rust_path, where)
        if python_side is None:
            python_side = python_params(spec)
        arguments = any(param.name == self.rules.arguments for param in python_side)
        rust_side = self.rust.params(found.data, found.owner, arguments=arguments)
        rust_side = parameters.renamed(rust_side, renames or {})
        sides = parameters.Sides(where, rust_path, pairs)
        parameters.compare(self, sides, python_side, rust_side, concrete=concrete)

    def paired_member(self, entry: Entry, python: str, rust_item: str) -> None:
        """A member an entry pairs: a name finding, unless the entry maps."""
        self.applied.add(id(entry))
        if entry.table == "map":
            return
        member = entry.python.rpartition(".")[2]
        detail = f"named {member} in Python"
        self.find("rust", entry.rust, "name", detail, rust_item, python, qualifier=entry.python)

    def check_async(
        self, method: dict, owner: str, spec: list, pairs: tuple, where: str, python: str
    ) -> None:
        """Async on both sides or neither: Rust's `where` against Python's
        `python`, which the key names."""
        python_async = spec[0].startswith("async")
        rust_async = self.rust.is_async(method, owner)
        if python_async != rust_async:
            detail = (
                "async in Python, not in Rust" if python_async else "async in Rust, not in Python"
            )
            self.find("rust", where, "async", detail, *pairs, qualifier=python)

    def non_exhaustive(self, item: dict, python: str | None) -> None:
        """A struct a caller builds keeps no `#[non_exhaustive]`: outside its
        crate that refuses the struct update standing for Python's keywords
        (roomkit-rs convention 9)."""
        detail = "#[non_exhaustive]: a caller outside the crate cannot build it by struct update"
        self.find("rust", item["path"], "non_exhaustive", detail, item["path"], python)
