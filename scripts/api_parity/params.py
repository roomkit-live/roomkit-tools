"""A call's Python parameters against the Rust ones standing for them.

Each parameter is matched by name. What is left is read in three ways
before it is reported one by one: a Python `**keywords` standing for the
fields of the Rust struct a parameter or a builder of that name brings; a
Rust field holding an options struct whose fields Python takes flat; and a
single parameter left on each side, which is one parameter named twice.
"""

from __future__ import annotations

from dataclasses import replace
from typing import TYPE_CHECKING

from api_parity import defaults
from api_parity.rust import RustParam, base_type
from api_parity.sides import PythonParam

if TYPE_CHECKING:
    from api_parity.compare import Comparison


def compare(
    comparison: Comparison,
    where: str,
    python_side: list[PythonParam],
    rust_side: list[RustParam],
    pairs: tuple,
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
            _requiredness(comparison, param, found, pairs)
    rust_left = [r for r in rust_side if r.path not in taken]
    left, rust_left = _variadic(comparison, where, left, rust_left, pairs)
    left, rust_left = _nested(comparison, left, rust_left, pairs)
    plain = [param for param in left if not param.variadic]
    if len(plain) == 1 and len(rust_left) == 1 and len(left) == 1:
        detail = f"the one parameter left on each side: named {plain[0].name} in Python"
        comparison.find("rust", rust_left[0].path, "name", detail, *pairs)
        if concrete:
            _requiredness(comparison, plain[0], rust_left[0], pairs)
        return
    for param in left:
        comparison.find(
            "python", f"{where}({param.name})", "unmatched", "no Rust parameter", *pairs
        )
    for param in rust_left:
        comparison.find("rust", param.path, "unmatched", "no Python parameter", *pairs)


def renamed(rust_side: list[RustParam], renames: dict[str, str]) -> list[RustParam]:
    """Rust parameters under the Python names an approved `[[map]]` entry's
    `params` gives them."""
    return [replace(param, name=renames.get(param.name, param.name)) for param in rust_side]


def _requiredness(
    comparison: Comparison, param: PythonParam, found: RustParam, pairs: tuple
) -> None:
    if param.required and not found.required:
        comparison.find(
            "rust", found.path, "required", "Python requires it; Rust defaults it", *pairs
        )
    elif not param.required and found.required and not _optional(comparison, param, found):
        detail = f"Python defaults it ({param.name}={param.default}); Rust requires it"
        comparison.find("rust", found.path, "defaulted", detail, *pairs)
    elif not param.required and not found.required:
        defaults.compare(comparison, param, found, pairs)


def _optional(comparison: Comparison, param: PythonParam, found: RustParam) -> bool:
    """`[rules] optional`: Rust has no optional call arguments, so an
    `Option<T>` parameter stands for Python's `=None`."""
    rules = comparison.rules
    return param.default == rules.setting("optional", "default") and base_type(
        found.type
    ) == rules.setting("optional", "type")


def _variadic(
    comparison: Comparison,
    where: str,
    left: list[PythonParam],
    rust_left: list[RustParam],
    pairs: tuple,
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
        comparison.find("python", f"{where}({param.name})", "variadic", detail, *pairs)
        rust_left = [r for r in rust_left if r not in group]
    return still, rust_left


def _nested(
    comparison: Comparison, left: list[PythonParam], rust_left: list[RustParam], pairs: tuple
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
        detail = f"nests {', '.join(names)} in a {short}; Python takes them flat"
        comparison.find("rust", param.path, "form", detail, *pairs)
        left = [p for p in left if p not in flat]
        still.remove(param)
    return left, still
