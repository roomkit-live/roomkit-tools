"""An error enum against its Python base class's family.

Each variant `X` is the class `<Stem>XError`, or `XError`, of the classes
deriving from the base, in Python's acronym casing (`Io`, `ToolIOError`),
`<Stem>` the base's name without `Error`; or the builtin exception `[rules]
builtin_errors` names. A class that takes a message alone has its variant's
payload compose it, so a payload is not compared. A variant has no member:
any member of the base or of a family class is a finding, but its bases and
an `__init__` taking the message alone.
"""

from __future__ import annotations

from typing import TYPE_CHECKING

from api_parity.sides import python_params

if TYPE_CHECKING:
    from api_parity.compare import Comparison


def compare(comparison: Comparison, item: dict, python: str) -> None:
    family = [path for path in comparison.python.specs if python in comparison.python.bases(path)]
    stem = python.rpartition(".")[2].removesuffix("Error")
    matched: set[str] = set()
    for variant in item.get("variants", []):
        path = f"{item['path']}::{variant['name']}"
        if path in comparison.rules.builtin_errors:
            comparison.builtin_errors_read.add(path)
            continue
        found = _exception(comparison, family, stem, variant["name"])
        if found is None:
            detail = f"an error variant: no Python exception class of the {python} family"
            comparison.find("rust", path, "unmatched", detail, item["path"], python)
        else:
            matched.add(found)
    for path in family:
        comparison.consumed.add(path)
        if path not in matched:
            detail = f"an exception of the {python} family: no Rust variant of {item['path']}"
            comparison.find("python", path, "unmatched", detail, item["path"], python)
    for path in [python, *family]:
        _members(comparison, item, python, path)
    for method in item.get("methods", []):
        comparison.find(
            "rust",
            f"{item['path']}::{method['name']}",
            "unmatched",
            "no Python member",
            item["path"],
            python,
        )


def _members(comparison: Comparison, item: dict, python: str, path: str) -> None:
    """A family class's own members: an error variant has none."""
    for member, spec in comparison.python.specs.get(path, {}).items():
        if member == "__bases__" or (member == "__init__" and _message_alone(spec)):
            continue
        detail = f"a member of an exception of the {python} family: a Rust variant has none"
        comparison.find("python", f"{path}.{member}", "unmatched", detail, item["path"], python)


def _message_alone(spec: list) -> bool:
    params = python_params(spec)
    return len(params) == 1 and params[0].required


def _exception(comparison: Comparison, family: list[str], stem: str, variant: str) -> str | None:
    for wanted in (f"{stem}{variant}Error", f"{variant}Error"):
        name = comparison.rules.python_type_name(wanted)
        found = next((path for path in family if path.rpartition(".")[2] == name), None)
        if found is not None:
            return found
    return None
