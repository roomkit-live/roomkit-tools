"""A call's Python parameters against the Rust ones standing for them.

Each parameter is matched by name. What is left is read in three ways
before it is reported one by one: a Python `**keywords` standing for the
fields of the Rust struct a parameter or a builder of that name brings; a
Rust field holding an options struct whose fields Python takes flat; and a
single parameter left on each side, which is one parameter named twice.

A finding's key names what it was compared with (`Finding.qualifier`): a
parameter on one side only, the other side's call; a parameter named twice,
its Python name; a `variadic` or nested `form` finding, the struct and the
fields it stands for. An entry covering it is dead once that changes.
"""

from __future__ import annotations

from dataclasses import dataclass, replace
from typing import TYPE_CHECKING

from api_parity import defaults
from api_parity.rust import RustParam, base_type
from api_parity.sides import PythonParam

if TYPE_CHECKING:
    from api_parity.compare import Comparison


@dataclass(frozen=True)
class Sides:
    """The two calls compared: Python's path, Rust's path, and the pair of
    items a finding names."""

    python: str
    rust: str
    pairs: tuple[str | None, str | None]

    def param(self, param: PythonParam) -> str:
        """A Python parameter as a key names it, with its default."""
        default = f"={param.default}" if param.default is not None else ""
        return f"{self.python}({param.name}{default})"


def compare(
    comparison: Comparison,
    sides: Sides,
    python_side: list[PythonParam],
    rust_side: list[RustParam],
    *,
    concrete: bool,
) -> None:
    """Each parameter on both sides, and, `concrete`, required on both or
    defaulted on both, to the same value (`defaults`). An interface (a trait
    against a protocol) compares no default: its implementation receives
    every argument."""
    left: list[PythonParam] = []
    taken: set[str] = set()
    for param in python_side:
        found = (
            None
            if param.variadic
            else next((r for r in rust_side if r.path not in taken and r.name == param.name), None)
        )
        if found is None:
            left.append(param)
            continue
        taken.add(found.path)
        if concrete:
            _requiredness(comparison, sides, param, found)
    rust_left = [r for r in rust_side if r.path not in taken]
    left, rust_left = _variadic(comparison, sides, left, rust_left)
    left, rust_left = _nested(comparison, sides, left, rust_left)
    _left(comparison, sides, left, rust_left, concrete=concrete)


def _left(
    comparison: Comparison,
    sides: Sides,
    left: list[PythonParam],
    rust_left: list[RustParam],
    *,
    concrete: bool,
) -> None:
    """What no name matched: one parameter named twice, or each on one side."""
    plain = [param for param in left if not param.variadic]
    if len(plain) == 1 and len(rust_left) == 1 and len(left) == 1:
        detail = f"the one parameter left on each side: named {plain[0].name} in Python"
        comparison.find(
            "rust", rust_left[0].path, "name", detail, *sides.pairs, qualifier=sides.param(plain[0])
        )
        if concrete:
            _requiredness(comparison, sides, plain[0], rust_left[0])
        return
    for param in left:
        comparison.find(
            "python",
            f"{sides.python}({param.name})",
            "unmatched",
            "no Rust parameter",
            *sides.pairs,
            qualifier=sides.rust,
        )
    for param in rust_left:
        comparison.find(
            "rust",
            param.path,
            "unmatched",
            "no Python parameter",
            *sides.pairs,
            qualifier=sides.python,
        )


def renamed(rust_side: list[RustParam], renames: dict[str, str]) -> list[RustParam]:
    """Rust parameters under the Python names an approved `[[map]]` entry's
    `params` gives them."""
    return [replace(param, name=renames.get(param.name, param.name)) for param in rust_side]


def _requiredness(
    comparison: Comparison, sides: Sides, param: PythonParam, found: RustParam
) -> None:
    where = sides.param(param)
    if param.required and not found.required:
        comparison.find(
            "rust",
            found.path,
            "required",
            "Python requires it; Rust defaults it",
            *sides.pairs,
            qualifier=where,
        )
    elif not param.required and found.required and not _optional(comparison, param, found):
        detail = f"Python defaults it ({param.name}={param.default}); Rust requires it"
        comparison.find("rust", found.path, "defaulted", detail, *sides.pairs, qualifier=where)
    elif not param.required and not found.required:
        defaults.compare(comparison, param, found, sides.pairs, where)


def _optional(comparison: Comparison, param: PythonParam, found: RustParam) -> bool:
    """`[rules] optional`: Rust has no optional call arguments, so an
    `Option<T>` parameter stands for Python's `=None`."""
    rules = comparison.rules
    return param.default == rules.setting("optional", "default") and base_type(
        found.type
    ) == rules.setting("optional", "type")


def _variadic(
    comparison: Comparison,
    sides: Sides,
    left: list[PythonParam],
    rust_left: list[RustParam],
) -> tuple[list[PythonParam], list[RustParam]]:
    """A Python `**name` against the fields of the Rust struct `name` brings:
    one finding naming them."""
    still: list[PythonParam] = []
    for param in left:
        group = [
            r
            for r in rust_left
            if param.variadic == "**" and r.via is not None and r.group == param.name
        ]
        if not group:
            still.append(param)
            continue
        fields = ", ".join(r.name for r in group)
        via = group[0].via.rpartition("::")[2]
        detail = f"takes **{param.name}; Rust's {via} names {fields}"
        comparison.find(
            "python",
            f"{sides.python}({param.name})",
            "variadic",
            detail,
            *sides.pairs,
            qualifier=f"{sides.rust}: {via}({fields})",
        )
        rust_left = [r for r in rust_left if r not in group]
    return still, rust_left


def _nested(
    comparison: Comparison,
    sides: Sides,
    left: list[PythonParam],
    rust_left: list[RustParam],
) -> tuple[list[PythonParam], list[RustParam]]:
    """A Rust field holding an options struct whose fields Python takes
    flat: one finding naming them."""
    still = list(rust_left)
    for param in rust_left:
        if param.via is None:
            continue
        nested = comparison.surface.named("struct", base_type(param.type), param.path)
        if nested is None or not comparison.rust.is_options(nested):
            continue
        names = [field["name"] for field in nested.get("fields", [])]
        flat = [p for p in left if p.name in names]
        if not names or len(flat) != len(names):
            continue
        comparison.rust.expanded.add(nested["path"])
        short = nested["path"].rpartition("::")[2]
        fields = ", ".join(names)
        detail = f"nests {fields} in a {short}; Python takes them flat"
        comparison.find(
            "rust",
            param.path,
            "form",
            detail,
            *sides.pairs,
            qualifier=f"{sides.python}: {short}({fields})",
        )
        left = [p for p in left if p not in flat]
        still.remove(param)
    return left, still
