"""Python's default values against the Rust structs' own.

rustdoc shows no default value, so a test writes the Rust side
(`examples/tests/api_defaults.rs`, `make api-parity`): each struct a
call's keywords stand for, built as a caller builds it (`Default`, or `new`
with its required fields), each field as JSON. A parameter both sides
default is compared with the field or the builder it stands for:
- the same JSON value is no difference, a Python tuple being a list;
- Python's `None` for a field that is no `Option` is the Rust default:
  Python passes no value and Rust's applies (`[rules] deferred_default`);
- a builder Rust does not call is Python's `None` (`[rules] builder_prefix`);
- any other pair is a `default` finding. An `Option` Rust resolves at
  execution is one too: to what it resolves is not read mechanically, so it
  is an entry's to approve, never a silent pass.
A struct the comparison reads that the dump lacks, or one of its fields, is
a `dump` finding, which no entry covers: the dump grows with the surface.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import TYPE_CHECKING, Any

from api_parity import ParityError, values
from api_parity.rust import RustParam, base_type
from api_parity.sides import PythonParam

if TYPE_CHECKING:
    from api_parity.compare import Comparison

DUMP = "examples/tests/api_defaults.rs"
# A Python default no literal reads (`<class 'list'>`).
_UNREAD = values.UNREAD


class Defaults:
    """The Rust defaults dump: by struct path, each field's JSON value."""

    def __init__(self, structs: dict[str, dict[str, Any]]) -> None:
        self.structs = structs


def load_defaults(path: Path) -> Defaults:
    try:
        structs = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        raise ParityError(f"{path}: the Rust defaults cannot be read: {error}") from error
    if not isinstance(structs, dict) or not all(isinstance(v, dict) for v in structs.values()):
        raise ParityError(f"{path}: the Rust defaults are a table of structs' fields")
    return Defaults(structs)


def compare(
    comparison: Comparison, param: PythonParam, found: RustParam, pairs: tuple, where: str
) -> None:
    """A parameter both sides default: Python's value against the Rust
    default of the field, or the builder, it stands for. The finding's key
    names both values (`where` is the Python parameter with its default)."""
    if comparison.defaults is None or param.variadic:
        return
    if found.via is None:
        _builder(comparison, param, found, pairs, where)
        return
    rust = _dumped(comparison, found, pairs)
    if rust is _UNREAD:
        return
    python = values.python_value(param.default)
    if python is not _UNREAD and values.same(python, rust):
        return
    if python is None and _deferred(comparison, param, found):
        return
    detail = _detail(param, found, rust, python)
    qualifier = f"{where}, Rust {json.dumps(rust, sort_keys=True)}"
    comparison.find("rust", found.path, "default", detail, *pairs, qualifier=qualifier)


def _dumped(comparison: Comparison, found: RustParam, pairs: tuple) -> Any:
    """The field's default in the dump, or a `dump` finding and `_UNREAD`."""
    struct = comparison.defaults.structs.get(found.via)
    field = found.path.rpartition("::")[2]
    if struct is None:
        detail = f"no defaults in the Rust dump ({DUMP}): add the struct there"
        comparison.find("rust", found.via, "dump", detail, *pairs)
        return _UNREAD
    if field not in struct:
        detail = f"no default in the Rust dump ({DUMP}): add the field there"
        comparison.find("rust", found.path, "dump", detail, *pairs)
        return _UNREAD
    return struct[field]


def _builder(
    comparison: Comparison, param: PythonParam, found: RustParam, pairs: tuple, where: str
) -> None:
    """A builder Rust does not call by default stands for Python's `None`."""
    if param.default != "None":
        detail = (
            f"Python defaults it to {param.default}; Rust's builder is not called by "
            "default, its value unread"
        )
        comparison.find("rust", found.path, "default", detail, *pairs, qualifier=where)


def _deferred(comparison: Comparison, param: PythonParam, found: RustParam) -> bool:
    """`[rules] deferred_default`: Python's `None` for a field that is no
    `Option` passes no value, and the Rust default applies."""
    rules = comparison.rules
    deferred = rules.setting("deferred_default", "python")
    return param.default == deferred and base_type(found.type) != "Option"


def _detail(param: PythonParam, found: RustParam, rust: Any, python: Any) -> str:
    if python is _UNREAD:
        return f"Python's default {param.default} is no literal the gate reads"
    if rust is None and base_type(found.type) == "Option":
        return (
            f"Python defaults it to {param.default}; Rust's is None, which it resolves "
            "at execution: to what is not read"
        )
    return f"Python defaults it to {param.default}; Rust to {json.dumps(rust)}"
