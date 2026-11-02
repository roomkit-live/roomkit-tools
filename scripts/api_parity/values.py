"""Literal values on both sides: a Python repr against a Rust literal.

rustdoc shows an associated constant's value as its literal is written
(`1_024`, `"kit"`, `true`), and none for an expression. A Python value is a
`repr` the snapshot holds. Two values are the same when both read and their
JSON is the same: `True` is no `1`, nor `1` a `1.0`.
"""

from __future__ import annotations

import ast
import json
import re
from typing import Any

# A Python value no literal reads, or a Rust value that is no literal.
UNREAD = object()
_NUMBER = re.compile(
    r"(-?[0-9][0-9_]*(?:\.[0-9_]+)?(?:[eE][+-]?[0-9]+)?)(?:[iuf](?:8|16|32|64|128|size))?"
)
_STRING = re.compile(r'"(?:[^"\\]|\\.)*"')


def python_value(text: str | None) -> Any:
    """A Python repr as the JSON value it stands for, or UNREAD."""
    try:
        return as_json(ast.literal_eval(text or ""))
    except (ValueError, SyntaxError, TypeError, MemoryError, RecursionError):
        return UNREAD


def rust_value(text: str | None) -> Any:
    """A Rust literal as the JSON value it stands for, or UNREAD."""
    text = (text or "").strip()
    if text in ("true", "false"):
        return text == "true"
    number = _NUMBER.fullmatch(text)
    if number:
        return python_value(number.group(1).replace("_", ""))
    if _STRING.fullmatch(text):
        return python_value(text)
    return UNREAD


def as_json(value: Any) -> Any:
    if isinstance(value, tuple | list):
        return [as_json(item) for item in value]
    if isinstance(value, dict):
        return {key: as_json(item) for key, item in value.items()}
    return value


def same(python: Any, rust: Any) -> bool:
    """The same JSON: `True` is no `1`, nor `1` a `1.0`."""
    try:
        return json.dumps(python, sort_keys=True) == json.dumps(rust, sort_keys=True)
    except TypeError:
        return False


def same_literal(python: str | None, rust: str | None) -> bool:
    """A Python repr and a Rust literal of the same value, both read."""
    left, right = python_value(python), rust_value(rust)
    return left is not UNREAD and right is not UNREAD and same(left, right)
