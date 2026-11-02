"""What no Python name stands for: the items, options structs and rules
the comparison left, and the Python names it compared with nothing."""

from __future__ import annotations

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from api_parity.compare import Comparison


def sweep(comparison: Comparison) -> None:
    _options(comparison)
    _builtins(comparison)
    _unmatched(comparison)
    _stale_rules(comparison)


def _options(comparison: Comparison) -> None:
    """An options struct a compared call expands stands for Python's
    keywords; one no call expands has no Python name."""
    for path, role in comparison.roles.items():
        if role != "options":
            continue
        item = comparison.surface.items[path]
        if path not in comparison.rust.expanded:
            comparison.find(
                "rust",
                path,
                "unmatched",
                "no Python name: no compared call takes it",
                path,
                None,
            )
            continue
        if item.get("non_exhaustive"):
            comparison.non_exhaustive(item, None)
        for member in item.get("methods", []) + item.get("constants", []):
            member_path = f"{path}::{member['name']}"
            if member_path not in comparison.rust.consumed:
                detail = "an options struct's own member: no Python counterpart"
                comparison.find("rust", member_path, "unmatched", detail, path, None)


def _builtins(comparison: Comparison) -> None:
    """A type standing for a Python builtin has no member Python has."""
    for path, builtin in comparison.rules.builtins.items():
        item = comparison.surface.items.get(path)
        for method in (item or {}).get("methods", []):
            detail = f"a member of a type standing for Python's {builtin}: no Python counterpart"
            comparison.find("rust", f"{path}::{method['name']}", "unmatched", detail, path, builtin)


def _unmatched(comparison: Comparison) -> None:
    for path in comparison.surface.items:
        if path not in comparison.roles:
            comparison.find("rust", path, "unmatched", "no Python name", path, None)
    for path in comparison.python.names():
        if path not in comparison.consumed:
            comparison.find("python", path, "unmatched", "no Rust item", None, path)


def _stale_rules(comparison: Comparison) -> None:
    """A rule naming a Rust item or variant that is gone, or that no
    comparison reads, is a finding no entry covers."""
    for path in comparison.rules.builtins:
        if path not in comparison.surface.items:
            comparison.find(
                "rust",
                path,
                "rules",
                "listed in [rules] builtins: no such Rust item",
                path,
                None,
            )
    for path in comparison.rules.builtin_errors:
        if path not in comparison.builtin_errors_read:
            detail = "listed in [rules] builtin_errors: no compared error enum has the variant"
            comparison.find("rust", path, "rules", detail, path.rpartition("::")[0], None)
