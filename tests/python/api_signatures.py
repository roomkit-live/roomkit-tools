"""The public Python API of an installed package, as a snapshot, and its drift.

The snapshot is the reference ``scripts/api-parity.py`` compares the Rust
crates with: Python is the reference implementation. Three files hold it, in
the snapshot's directory:

- ``public_api_names.json``: every name each public module exports, by
  module. A module's names are its ``__all__``, or, without one, the public
  classes and functions it defines itself. A module is public when no part of
  its dotted name starts with ``_``.
- ``public_api_signatures.json``: every class and function the package
  defines and a public module exports, keyed by the shortest path it is
  exported under: a function's parameters, a class's constructor, its public
  methods and properties, its public bases from the package, an enum's
  members. A parameter is written as ``inspect`` writes it, without its
  annotation: ``name``, ``name=default``, ``*args``, ``**kwargs``, and the
  ``/`` and ``*`` markers. A member's first item says what it is: ``def``,
  ``async def``, ``classmethod``, ``staticmethod`` (each possibly ``async``),
  ``property``, ``member`` or ``bases``. A protocol has no constructor.
- ``public_api_contracts.json`` (the base package only): the JSON shape of
  every tool contract a public class returns from ``definition()``. The
  schemas are derived from the Rust types; each named type is kept as its
  property names, its values (an enumeration) or its tagged variants (a
  tagged union), with neither descriptions nor requiredness.

``test_public_api.py`` fails when the code and the files differ, a
compatible addition included: the files are the exact public surface the
Rust crates mirror. The commit that changes the surface rewrites them with
``make api-snapshot``, which runs this module against freshly built wheels:

    python api_signatures.py write roomkit_tools DIRECTORY [--contracts]

Annotations are left out: whether a type widens or narrows is not read
mechanically. What a call does is not here either.
"""

from __future__ import annotations

import enum
import importlib
import inspect
import json
import pkgutil
import re
import sys
from dataclasses import dataclass
from functools import cached_property
from pathlib import Path
from types import ModuleType
from typing import Any

NAMES = "public_api_names.json"
SIGNATURES = "public_api_signatures.json"
CONTRACTS = "public_api_contracts.json"

_P = inspect.Parameter
_ADDRESS = re.compile(r" at 0x[0-9a-fA-F]+")

# The tool definitions a static `definition(name)` serves, by class name: the
# names are not discoverable from the signature.
NAMED_DEFINITIONS = {
    "ProcessSession": (
        "process_start",
        "process_status",
        "process_read",
        "process_write",
        "process_stop",
    ),
    "OutputStore": ("output_read", "output_forget"),
}

Spec = list[str]


@dataclass(frozen=True)
class Package:
    """An installed top-level package whose public modules are snapshotted."""

    name: str

    def modules(self) -> list[ModuleType]:
        """The package and each public module beneath it, imported."""
        root = importlib.import_module(self.name)
        found = [root]
        prefix = f"{self.name}."
        for info in pkgutil.walk_packages(root.__path__, prefix):
            if any(part.startswith("_") for part in info.name.split(".")):
                continue
            found.append(importlib.import_module(info.name))
        return sorted(found, key=lambda module: module.__name__)

    def owns(self, obj: Any) -> bool:
        """Whether ``obj`` is a class or function the package defines."""
        if not (inspect.isclass(obj) or inspect.isfunction(obj)):
            return False
        module = getattr(obj, "__module__", None) or ""
        return module == self.name or module.startswith(f"{self.name}.")


def exported_names(package: Package) -> dict[str, list[str]]:
    """Every name each public module exports, sorted, by module."""
    names: dict[str, list[str]] = {}
    for module in package.modules():
        exported = sorted(_exports(module, package))
        if exported:
            names[module.__name__] = exported
    return names


def build(package: Package) -> dict[str, dict[str, Spec]]:
    """The signatures, keyed by the shortest path each object is exported under."""
    paths: dict[int, str] = {}
    objects: dict[int, Any] = {}
    for module in package.modules():
        for name in _exports(module, package):
            obj = getattr(module, name)
            if not package.owns(obj):
                continue
            path = f"{module.__name__}.{name}"
            known = paths.get(id(obj))
            if known is None or (path.count("."), path) < (known.count("."), known):
                paths[id(obj)] = path
                objects[id(obj)] = obj
    ordered = sorted(paths, key=paths.__getitem__)
    return {paths[key]: describe(objects[key], package, paths) for key in ordered}


def describe(obj: Any, package: Package, paths: dict[int, str]) -> dict[str, Spec]:
    """A class's or a function's members, each as the snapshot writes it."""
    if not inspect.isclass(obj):
        spec = _callable_spec(obj, "def", drop_first=False)
        return {} if spec is None else {"__call__": spec}
    members: dict[str, Spec] = {}
    bases = [paths[id(base)] for base in obj.__bases__ if id(base) in paths]
    if bases:
        members["__bases__"] = ["bases", *bases]
    if issubclass(obj, enum.Enum):
        members.update({name: ["member", _repr(m.value)] for name, m in obj.__members__.items()})
    elif _builds_itself(obj, package):
        try:
            members["__init__"] = ["def", *_tokens(inspect.signature(obj), drop_first=False)]
        except (TypeError, ValueError):
            pass
    for klass in obj.__mro__:
        if not package.owns(klass):
            continue
        for name, value in vars(klass).items():
            if name.startswith("_") or name in members:
                continue
            spec = _member_spec(value)
            if spec is not None:
                members[name] = spec
    return dict(sorted(members.items()))


def contracts(package: Package, signatures: dict[str, dict[str, Spec]]) -> dict[str, Any]:
    """The JSON shape of every tool contract a public class returns."""
    tools: dict[str, dict[str, str | None]] = {}
    types: dict[str, dict[str, Any]] = {}
    for path, members in signatures.items():
        module, _, name = path.rpartition(".")
        cls = getattr(importlib.import_module(module), name)
        for definition in _definitions(cls, members):
            tool = definition["name"]
            if tool in tools:
                continue
            tools[tool] = {
                "class": path,
                "input": _collect(definition["input_schema"], types, tool),
                "output": _collect(definition["output_schema"], types, tool),
            }
    return {"tools": dict(sorted(tools.items())), "types": dict(sorted(types.items()))}


def _definitions(cls: type, members: dict[str, Spec]) -> list[dict[str, Any]]:
    """The definitions a class serves: `definition()` taking nothing, or
    `definition(name)` for each name NAMED_DEFINITIONS lists."""
    spec = members.get("definition")
    if spec is None or spec[0] not in ("staticmethod", "classmethod"):
        return []
    if len(spec) == 1:
        return [cls.definition()]
    names = NAMED_DEFINITIONS.get(cls.__name__)
    if names is None:
        raise ValueError(f"{cls.__name__}.definition: list its tool names in NAMED_DEFINITIONS")
    return [cls.definition(name) for name in names]


def _collect(schema: dict[str, Any], types: dict[str, dict[str, Any]], tool: str) -> str | None:
    """Add the schema's named types to ``types``; return the root's name."""
    named = dict(schema.get("$defs", {}))
    root = schema.get("title")
    if root:
        named[root] = schema
    for name, body in named.items():
        shape = _shape(body)
        if types.setdefault(name, shape) != shape:
            raise ValueError(f"{tool}: the JSON type {name} differs from another contract's")
    return root


def _shape(schema: dict[str, Any]) -> dict[str, Any]:
    """A named JSON type's property names, values or tagged variants."""
    if "properties" in schema or (schema.get("type") == "object" and "oneOf" not in schema):
        return {"fields": sorted(schema.get("properties", {}))}
    if "enum" in schema:
        return {"values": list(schema["enum"])}
    variants = schema.get("oneOf") or schema.get("anyOf") or []
    if variants and all("const" in variant for variant in variants):
        return {"values": [variant["const"] for variant in variants]}
    if variants and all("properties" in variant for variant in variants):
        tags = set.intersection(
            *(
                {key for key, value in variant["properties"].items() if "const" in value}
                for variant in variants
            )
        )
        if len(tags) == 1:
            (tag,) = tags
            return {
                "tag": tag,
                "variants": {
                    variant["properties"][tag]["const"]: sorted(set(variant["properties"]) - {tag})
                    for variant in variants
                },
            }
    if schema.get("type") == "array":
        return {"items": schema.get("items", {}).get("$ref", "").rpartition("/")[2]}
    raise ValueError(f"a JSON type this snapshot cannot read: {sorted(schema)}")


def drift(directory: Path, package: Package, *, with_contracts: bool) -> list[str]:
    """Where the code differs from the snapshot in ``directory``, both ways."""
    found: list[str] = []
    held_names = _read(directory / NAMES)
    names = exported_names(package)
    for module in sorted(set(held_names) | set(names)):
        before, after = set(held_names.get(module, ())), set(names.get(module, ()))
        found += [
            f"{module}.{name}: exported, not in the snapshot" for name in sorted(after - before)
        ]
        found += [
            f"{module}.{name}: in the snapshot, no longer exported"
            for name in sorted(before - after)
        ]
    held = _read(directory / SIGNATURES)
    signatures = build(package)
    for path in sorted(set(held) | set(signatures)):
        before, after = held.get(path), signatures.get(path)
        if before is None or after is None:
            found.append(
                f"{path}: " + ("not in the snapshot" if before is None else "no longer exported")
            )
            continue
        for member in sorted(set(before) | set(after)):
            if before.get(member) != after.get(member):
                where = path if member == "__call__" else f"{path}.{member}"
                found.append(
                    f"{where}: {before.get(member)} in the snapshot, "
                    f"{after.get(member)} in the code"
                )
    if with_contracts:
        held_contracts = _read(directory / CONTRACTS)
        current = contracts(package, signatures)
        for table in ("tools", "types"):
            before, after = held_contracts.get(table, {}), current[table]
            for name in sorted(set(before) | set(after)):
                if before.get(name) != after.get(name):
                    found.append(
                        f"contract {table} {name}: {before.get(name)} in the snapshot, "
                        f"{after.get(name)} in the code"
                    )
    return found


def write(directory: Path, package: Package, *, with_contracts: bool) -> None:
    """Rewrite the snapshot in ``directory`` from the installed code."""
    signatures = build(package)
    (directory / NAMES).write_text(
        json.dumps(exported_names(package), indent=2) + "\n", encoding="utf-8"
    )
    blocks = []
    for path, members in signatures.items():
        lines = [
            f"    {json.dumps(name)}: {json.dumps(spec, ensure_ascii=False)}"
            for name, spec in members.items()
        ]
        body = "{\n" + ",\n".join(lines) + "\n  }" if lines else "{}"
        blocks.append(f"  {json.dumps(path)}: {body}")
    (directory / SIGNATURES).write_text("{\n" + ",\n".join(blocks) + "\n}\n", encoding="utf-8")
    if with_contracts:
        text = json.dumps(contracts(package, signatures), indent=1, ensure_ascii=False)
        (directory / CONTRACTS).write_text(text + "\n", encoding="utf-8")


def _read(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8")) if path.exists() else {}


def _exports(module: ModuleType, package: Package) -> list[str]:
    listed = getattr(module, "__all__", None)
    if listed is not None:
        return list(listed)
    return [
        name
        for name, value in vars(module).items()
        if not name.startswith("_") and package.owns(value) and value.__module__ == module.__name__
    ]


def _builds_itself(cls: type, package: Package) -> bool:
    """Whether the package decides how ``cls`` is constructed: an ``__init__``
    or ``__new__`` of its own. A protocol has no constructor, and one inherited
    from the standard library is left out (it varies between Python versions)."""
    if getattr(cls, "_is_protocol", False):
        return False
    for klass in cls.__mro__:
        if "__init__" in vars(klass) or "__new__" in vars(klass):
            return package.owns(klass)
    return False


def _member_spec(value: Any) -> Spec | None:
    if isinstance(value, property | cached_property):
        return ["property"]
    if isinstance(value, staticmethod):
        return _callable_spec(value.__func__, "staticmethod", drop_first=False)
    if isinstance(value, classmethod):
        return _callable_spec(value.__func__, "classmethod", drop_first=True)
    if inspect.isfunction(value):
        return _callable_spec(value, "def", drop_first=True)
    return None


def _callable_spec(fn: Any, binding: str, *, drop_first: bool) -> Spec | None:
    try:
        signature = inspect.signature(fn)
    except (TypeError, ValueError):
        return None
    flavour = (
        "async gen "
        if inspect.isasyncgenfunction(fn)
        else "async "
        if inspect.iscoroutinefunction(fn)
        else ""
    )
    return [flavour + binding, *_tokens(signature, drop_first=drop_first)]


def _tokens(signature: inspect.Signature, *, drop_first: bool) -> Spec:
    params = list(signature.parameters.values())
    if drop_first and params and params[0].kind in (_P.POSITIONAL_ONLY, _P.POSITIONAL_OR_KEYWORD):
        params = params[1:]
    tokens: Spec = []
    keyword_only = False
    for i, p in enumerate(params):
        if p.kind is _P.VAR_POSITIONAL:
            tokens.append(f"*{p.name}")
            keyword_only = True
            continue
        if p.kind is _P.VAR_KEYWORD:
            tokens.append(f"**{p.name}")
            continue
        if p.kind is _P.KEYWORD_ONLY and not keyword_only:
            tokens.append("*")
            keyword_only = True
        tokens.append(p.name if p.default is _P.empty else f"{p.name}={_repr(p.default)}")
        following = params[i + 1].kind if i + 1 < len(params) else None
        if p.kind is _P.POSITIONAL_ONLY and following is not _P.POSITIONAL_ONLY:
            tokens.append("/")
    return tokens


def _repr(value: Any) -> str:
    if isinstance(value, set | frozenset):
        return f"{type(value).__name__}({sorted(map(repr, value))})"
    return _ADDRESS.sub("", repr(value))


def main(argv: list[str]) -> int:
    if len(argv) not in (3, 4) or argv[0] != "write" or argv[3:] not in ([], ["--contracts"]):
        print("usage: api_signatures.py write PACKAGE DIRECTORY [--contracts]", file=sys.stderr)
        return 2
    write(Path(argv[2]), Package(argv[1]), with_contracts=argv[3:] == ["--contracts"])
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
