"""What a Rust item's members stand for on the Python side.

A Rust method's parameters are read as the Python parameters they stand
for: `&self` and the cancellation token drop out, a struct of named fields
(an `<X>Input`, or an options struct, `[rules] options_suffixes`) stands for
its fields, and a constructor's builders (`with_<x>(self, value)`) add their
own.
"""

from __future__ import annotations

import re
from dataclasses import dataclass

from api_parity.mapping import Rules
from api_parity.sides import Surface, member_of

_FUTURE = re.compile(r"\bFuture\s*<")


@dataclass(frozen=True)
class RustParam:
    """A Rust parameter, or a field of a struct standing for parameters, as
    the Python parameter it stands for."""

    name: str
    required: bool
    path: str  # where a finding on it points
    via: str | None = None  # the struct a field comes from
    group: str | None = None  # the parameter or builder that brings the struct
    whole: bool = False  # an `<X>Input` standing for a Python `arguments` dict
    type: str = ""


def base_type(rust_type: str) -> str:
    """`&'a mut path::Name<T>` as `Name`."""
    stripped = re.sub(r"^&(?:'\w+\s+)?(?:mut\s+)?", "", rust_type.strip())
    return re.split(r"[<\s]", stripped, maxsplit=1)[0].rpartition("::")[2]


def python_name(rust: str) -> str:
    """A Rust identifier as Python writes it: `r#type` is `type`, and an
    unused parameter's leading `_` is dropped."""
    return rust.removeprefix("r#").lstrip("_")


class RustSide:
    """The surface, read with the rules."""

    def __init__(self, surface: Surface, rules: Rules) -> None:
        self.surface = surface
        self.rules = rules
        # Structs whose fields stood for Python parameters, and the
        # constructors that set them.
        self.expanded: set[str] = set()
        self.consumed: set[str] = set()

    def item(self, path: str) -> dict | None:
        return self.surface.items.get(path)

    def is_wire(self, item: dict) -> bool:
        """A contract type: its JSON is what a Python call passes or returns."""
        return item["kind"] in ("struct", "enum") and "JsonSchema" in item.get("impls", [])

    def is_input(self, item: dict) -> bool:
        return item["kind"] == "struct" and item["path"].endswith(self.rules.input_suffix)

    def is_options(self, item: dict) -> bool:
        """A struct of named fields standing for keyword arguments."""
        name = item["path"].rpartition("::")[2]
        return (
            item["kind"] == "struct"
            and not self.is_input(item)
            and any(name.endswith(suffix) for suffix in self.rules.options_suffixes)
        )

    def is_future_alias(self, item: dict) -> bool:
        """A boxed future alias: how a trait object's method writes `async`."""
        return item["kind"] == "type" and bool(_FUTURE.search(item.get("aliased", "")))

    def is_async(self, method: dict, near: str) -> bool:
        """An `async fn`, or a function returning a future: `impl Future`, or
        a boxed future alias of the surface."""
        if method["async"]:
            return True
        returns = method.get("returns") or ""
        alias = self.surface.named("type", base_type(returns), near) if returns else None
        return alias is not None and self.is_future_alias(alias)

    def is_cancellation(self, param: dict) -> bool:
        return base_type(param["type"]) == self.rules.cancellation

    def struct_param(self, param: dict, near: str) -> dict | None:
        """The struct of named fields a parameter's type is, if any."""
        found = self.surface.named("struct", base_type(param["type"]), near)
        if found is None or not (self.is_input(found) or self.is_options(found)):
            return None
        return found

    def builders(self, item: dict) -> list[dict]:
        """`with_<x>(self, value)`: a constructor's optional settings."""
        return [
            method
            for method in item.get("methods", [])
            if method["name"].startswith(self.rules.builder_prefix)
            and (method["receiver"] or "").removeprefix("mut ") == "self"
            and len(method["params"]) == 1
        ]

    def method(self, item: dict, name: str, *, receiver: bool | None = None) -> dict | None:
        for method in item.get("methods", []):
            if method["name"] == name and (
                receiver is None or (method["receiver"] is not None) == receiver
            ):
                return method
        return None

    def params(self, method: dict, owner: str, *, arguments: bool) -> list[RustParam]:
        """A method's parameters as Python's: the cancellation token left
        out, a struct of named fields expanded. With `arguments`, Python
        passes an `<X>Input` whole, as its dict."""
        found: list[RustParam] = []
        for param in method["params"]:
            if self.is_cancellation(param):
                continue
            path = f"{owner}::{method['name']}({param['name']})"
            struct = self.struct_param(param, owner)
            if struct is not None and self.is_input(struct) and arguments:
                found.append(
                    RustParam(self.rules.arguments, True, path, whole=True, type=param["type"])
                )
            elif struct is not None:
                found += self.fields(struct, group=python_name(param["name"]), builder=False)
            else:
                found.append(RustParam(python_name(param["name"]), True, path, type=param["type"]))
        return found

    def constructor(self, item: dict, new: dict) -> list[RustParam]:
        """A constructor's parameters, and those its builders add."""
        found = self.params(new, item["path"], arguments=False)
        for builder in self.builders(item):
            self.consumed.add(f"{item['path']}::{builder['name']}")
            (param,) = builder["params"]
            struct = self.struct_param(param, item["path"])
            if struct is not None:
                found += self.fields(struct, group=python_name(param["name"]), builder=True)
            else:
                path = f"{item['path']}::{builder['name']}"
                found.append(RustParam(python_name(param["name"]), False, path, type=param["type"]))
        return found

    def fields(self, struct: dict, *, group: str | None, builder: bool) -> list[RustParam]:
        """A struct's fields as parameters. One its `new` takes is required;
        any other is set by struct update, which needs `new` or `Default`;
        a struct with neither is built by a literal setting every field.
        A builder's struct is optional whole, when it has `Default`."""
        self.expanded.add(struct["path"])
        new = self.method(struct, "new", receiver=False)
        default = "Default" in struct.get("impls", [])
        taken: set[str] = set()
        if new is not None:
            self.consumed.add(f"{struct['path']}::new")
            taken = {python_name(param["name"]) for param in new["params"]}
        found = []
        for field in struct.get("fields", []):
            name = python_name(field["name"])
            if builder and default:
                required = False
            elif new is not None:
                required = name in taken
            else:
                required = not default
            path = f"{struct['path']}::{field['name']}"
            found.append(
                RustParam(name, required, path, via=struct["path"], group=group, type=field["type"])
            )
        return found

    def trait_members(self, item: dict) -> dict[str, tuple[str, dict]]:
        """The methods of the surface's traits the item implements, by name,
        with the trait each comes from."""
        found: dict[str, tuple[str, dict]] = {}
        for implemented in item.get("impls", []):
            trait = self.surface.named(
                "trait", re.split(r"[<\s]", implemented, maxsplit=1)[0], item["path"]
            )
            if trait is None:
                continue
            for kind in ("required", "provided"):
                for method in trait.get(kind, []):
                    found.setdefault(method["name"], (trait["path"], method))
        return found

    def member(self, path: str) -> tuple[dict, dict] | None:
        """The item and the member a `crate::Item::member` path names."""
        base, member, _ = self.surface.split(path)
        item = self.surface.items.get(base)
        if item is None or member is None:
            return None
        found = member_of(item, member)
        return None if found is None else (item, found)
