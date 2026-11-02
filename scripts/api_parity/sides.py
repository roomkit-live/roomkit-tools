"""The two sides: the Rust surface and the Python snapshot."""

from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path

from api_parity import ParityError

NAMES = "public_api_names.json"
SIGNATURES = "public_api_signatures.json"
CONTRACTS = "public_api_contracts.json"
# The entries of a class's spec that are no member: its public bases, and the
# `__init__` parameters annotated with private native types only.
MARKERS = ("__bases__", "__native__")


def strip_param(path: str) -> tuple[str, str | None]:
    """`a.b(param)` as (`a.b`, `param`)."""
    if path.endswith(")") and "(" in path:
        head, _, param = path[:-1].partition("(")
        return head, param
    return path, None


@dataclass(frozen=True)
class PythonParam:
    """One parameter of a snapshot spec."""

    name: str
    required: bool
    default: str | None
    variadic: str  # "", "*" or "**"


def python_params(spec: list[str]) -> list[PythonParam]:
    """A member's parameters, without the `*` and `/` markers."""
    found = []
    for raw in spec[1:]:
        if raw in ("*", "/"):
            continue
        name, has_default, default = raw.partition("=")
        variadic = "**" if name.startswith("**") else "*" if name.startswith("*") else ""
        found.append(
            PythonParam(
                name.lstrip("*"), not has_default and not variadic, default or None, variadic
            )
        )
    return found


class Python:
    """The snapshot: every public name's spec, every module's exports, and
    the JSON shape of the tool contracts."""

    def __init__(self, signatures: dict, exports: dict, contracts: dict | None = None) -> None:
        self.specs: dict[str, dict[str, list[str]]] = signatures
        self.exports = {module: set(names) for module, names in exports.items()}
        self.contracts = contracts or {"tools": {}, "types": {}}
        self._described: dict[str, list[str]] = {}
        for path in signatures:
            self._described.setdefault(path.rpartition(".")[2], []).append(path)
        self._names: list[str] | None = None

    def find(self, modules: list[str], name: str) -> str | None:
        """The path of `name` exported by one of `modules`: the snapshot's,
        which is the shortest a name is exported under, or the export
        itself for a constant the snapshot does not describe."""
        for module in modules:
            path = f"{module}.{name}"
            if path in self.specs:
                return path
            if name in self.exports.get(module, ()):
                described = self._described.get(name, [])
                if len(described) > 1:
                    raise ParityError(
                        f"{path}: the snapshot describes {len(described)} objects of that name"
                    )
                return described[0] if described else self.constant(name)
        return None

    def constant(self, name: str) -> str:
        """A constant's path: the shortest module exporting it."""
        modules = sorted(
            (m for m, names in self.exports.items() if name in names),
            key=lambda m: (m.count("."), m),
        )
        return f"{modules[0]}.{name}"

    def names(self) -> list[str]:
        """Every Python name in scope: each described object, and each
        constant, by its shortest path."""
        if self._names is None:
            described = set(self._described)
            constants = {
                self.constant(name)
                for exported in self.exports.values()
                for name in exported
                if name not in described
            }
            self._names = sorted(set(self.specs) | constants)
        return self._names

    def bases(self, path: str) -> list[str]:
        return list(self.specs.get(path, {}).get("__bases__", ["bases"])[1:])

    def split(self, path: str) -> tuple[str, str | None, str | None]:
        """`pkg.Class.member(param)` as (name, member, param); a function's
        parameter is its `__call__`'s."""
        path, param = strip_param(path)
        if path in self.specs or path in self.names():
            return path, ("__call__" if param is not None else None), param
        head, _, last = path.rpartition(".")
        return (head, last, param) if head in self.specs else (path, None, param)

    def exists(self, path: str) -> bool:
        if path.startswith("json:"):
            return self._json_exists(path.removeprefix("json:"))
        base, member, param = self.split(path)
        if member is None:
            return param is None and base in self.names()
        spec = self.specs.get(base, {}).get(member)
        if spec is None:
            return False
        return param is None or param in [p.name for p in python_params(spec)]

    def _json_exists(self, path: str) -> bool:
        """`Type`, `Type.property` or `Type.variant.property` of a contract."""
        name, _, rest = path.partition(".")
        shape = self.contracts["types"].get(name)
        if shape is None:
            return False
        if not rest:
            return True
        if "fields" in shape:
            return rest in shape["fields"]
        if "values" in shape:
            return rest in shape["values"]
        variant, _, field = rest.partition(".")
        fields = shape.get("variants", {}).get(variant)
        return fields is not None and (not field or field in fields)


def native_params(spec: dict) -> list[str]:
    """The `__init__` parameters of a class's spec that take a native object:
    annotated with private native types only (`__native__`)."""
    return list(spec.get("__native__", ["native"])[1:])


class Surface:
    """The Rust surface `scripts/rs-surface.py` wrote, one or several crates' worth."""

    def __init__(self, written: list[dict]) -> None:
        self.rustdoc = sorted({surface.get("rustdoc", "") for surface in written})
        self.items: dict[str, dict] = {}
        for surface in written:
            for found in surface["crates"].values():
                for item in found:
                    self.items[item["path"]] = item
        self._named: dict[tuple[str, str], list[str]] = {}
        for path, item in self.items.items():
            self._named.setdefault((item["kind"], path.rpartition("::")[2]), []).append(path)

    def named(self, kind: str, name: str, near: str) -> dict | None:
        """The item of that kind and name, the one in `near`'s crate first."""
        found = self._named.get((kind, name), [])
        crate = near.split("::")[0]
        chosen = [path for path in found if path.split("::")[0] == crate] or found
        return self.items[chosen[0]] if chosen else None

    def split(self, path: str) -> tuple[str, str | None, str | None]:
        """`crate::Item::member(param)` as (item, member, param)."""
        path, param = strip_param(path)
        if path in self.items:
            return path, None, param
        head, _, last = path.rpartition("::")
        if head in self.items:
            return head, last, param
        owner, _, variant = head.rpartition("::")
        if owner in self.items:  # a variant's or a struct field's own field
            return owner, f"{variant}::{last}", param
        return path, None, param

    def exists(self, path: str) -> bool:
        base, member, param = self.split(path)
        item = self.items.get(base)
        if item is None:
            return False
        if member is None:
            return param is None or param in names(item.get("signature", {}).get("params", []))
        if "::" in member:
            variant, _, field = member.partition("::")
            found = member_of(item, variant)
            return found is not None and field in names(found.get("fields", []))
        found = member_of(item, member)
        if found is None:
            return False
        return (
            param is None
            or param in names(found.get("params", []))
            or param in names(found.get("fields", []))
        )


def names(entries: list[dict]) -> list[str]:
    return [entry["name"] for entry in entries]


def member_of(item: dict, name: str) -> dict | None:
    """A Rust item's field, method, trait method, constant or variant."""
    for kind in ("fields", "methods", "required", "provided", "constants", "variants"):
        for member in item.get(kind, []):
            if member["name"] == name:
                return member
    return None


def load_python(directories: list[Path]) -> Python:
    """The snapshots of every directory, merged."""
    signatures: dict = {}
    exports: dict = {}
    contracts: dict = {"tools": {}, "types": {}}
    for directory in directories:
        try:
            signatures.update(json.loads((directory / SIGNATURES).read_text(encoding="utf-8")))
            exports.update(json.loads((directory / NAMES).read_text(encoding="utf-8")))
            if (directory / CONTRACTS).exists():
                found = json.loads((directory / CONTRACTS).read_text(encoding="utf-8"))
                for table in ("tools", "types"):
                    contracts[table].update(found[table])
        except (OSError, ValueError, KeyError) as error:
            raise ParityError(
                f"{directory}: the Python snapshot cannot be read: {error}"
            ) from error
    if not signatures:
        raise ParityError("the Python snapshot holds no name")
    return Python(signatures, exports, contracts)


def load_surface(paths: list[Path]) -> Surface:
    written = []
    for path in paths:
        try:
            written.append(json.loads(path.read_text(encoding="utf-8")))
        except (OSError, ValueError) as error:
            raise ParityError(f"{path}: the surface cannot be read: {error}") from error
    try:
        surface = Surface(written)
    except (KeyError, TypeError) as error:
        raise ParityError(f"the surface cannot be read: {error}") from error
    if not surface.items:
        raise ParityError("the surface holds no item")
    return surface


def load_hidden(paths: list[Path]) -> list[str]:
    """The public paths `#[doc(hidden)]` hides, as `rs-surface.py --hidden`
    wrote them: the workspace's, and the RoomKit adapter's committed list."""
    hidden: list[str] = []
    for path in paths:
        try:
            written = json.loads(path.read_text(encoding="utf-8"))
            hidden += [str(item) for items in written["hidden"].values() for item in items]
        except (OSError, ValueError, KeyError, AttributeError, TypeError) as error:
            raise ParityError(f"{path}: the hidden items cannot be read: {error}") from error
    return sorted(set(hidden))
