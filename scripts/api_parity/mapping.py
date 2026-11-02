"""`api/mapping.toml`: the rules, and the entries a rule does not give."""

from __future__ import annotations

import re
import tomllib
from dataclasses import dataclass, field
from pathlib import Path

from api_parity import ParityError

_DATE = re.compile(r"\d{4}-\d{2}-\d{2}")
_TOKEN = re.compile(r"[A-Z][a-z0-9]*|[a-z0-9]+")
# A finding's key, as an entry's `covers` names it: side, path and kind.
_KEY = re.compile(r"(rust|python) (\S+) \(([a-z_]+)\)")
# A finding's kind: what differs.
KINDS = {
    "unmatched": "on one side only",
    "name": "named differently",
    "required": "Python requires it, Rust does not",
    "defaulted": "Python defaults it, Rust requires it",
    "async": "async on one side only",
    "form": "another form: property, method or function, flat or nested, built or wrapped",
    "variadic": "Python's **keywords stand for named Rust fields",
    "non_exhaustive": "a struct a caller builds keeps #[non_exhaustive]",
    "meaning": "the same name stands for another thing",
    "uncompared": "a contract type no tool contract holds",
    "result": "a call takes or returns another type than its contract or its class",
    "rules": "a rule names what is gone or never read",
}
# Each table's keys: those it requires, and those it may hold. `decision`
# names where the maintainer decided an entry.
_KEYS = {
    "map": ({"rust", "python", "approved", "reason"}, {"params", "decision"}),
    "exception": ({"covers", "approved", "reason"}, {"decision"}),
    "missing": ({"covers", "approved", "reason"}, {"decision"}),
    "pending": ({"covers", "reason"}, {"rust", "python"}),
}
# The kinds a `[[missing]]` entry covers: a feature one side lacks.
_MISSING_KINDS = {"unmatched", "uncompared"}
_RULES_REQUIRED = {
    "acronyms",
    "packages",
    "options_suffixes",
    "input_suffix",
    "arguments",
    "cancellation",
    "factory",
    "builder_prefix",
    "toolset",
}
# The rules a mapping may add as tables, each with its settings.
_TABLE_RULES = {
    "optional": {"type", "default"},
    "typed_call": {"method"},
    "named_calls": {"method", "key", "definition", "suffix"},
}
_RULES = _RULES_REQUIRED | {"builtins", "builtin_errors"} | set(_TABLE_RULES)


def tokens(name: str) -> list[str]:
    return _TOKEN.findall(name)


def snake_name(name: str) -> str:
    """A `CamelCase` variant as the `snake_case` value it serializes to."""
    return "_".join(token.lower() for token in tokens(name))


@dataclass(frozen=True)
class Toolset:
    """The RoomKit engine's tool protocol: a Rust struct implementing
    `trait` stands for the Python RoomKit tool named without `suffix`, and
    the trait's implementation for the Python `members`."""

    trait: str
    suffix: str
    members: tuple[str, ...]


@dataclass
class Rules:
    acronyms: list[tuple[list[str], str]]
    packages: dict[str, list[str]]
    options_suffixes: tuple[str, ...]
    input_suffix: str
    arguments: str
    cancellation: str
    factory: str
    builder_prefix: str
    toolset: Toolset
    # By the Rust type's path: the Python builtin it stands for.
    builtins: dict[str, str] = field(default_factory=dict)
    # By the error variant's path: the Python builtin exception it stands for.
    builtin_errors: dict[str, str] = field(default_factory=dict)
    # By rule: where the maintainer decided it, a rule table's `decision`.
    decisions: dict[str, str] = field(default_factory=dict)
    # The table rules the mapping holds (`_TABLE_RULES`), by name: their settings.
    tables: dict[str, dict[str, str]] = field(default_factory=dict)

    @classmethod
    def from_table(cls, table: dict) -> Rules:
        if not _RULES_REQUIRED <= set(table) <= _RULES:
            missing = sorted(_RULES_REQUIRED - set(table))
            unknown = sorted(set(table) - _RULES)
            raise ParityError(f"[rules]: missing {missing}, unknown {unknown}")
        acronyms = sorted(
            ((tokens(rust), python) for rust, python in _plain(table["acronyms"]).items()),
            key=lambda pair: -len(pair[0]),
        )
        toolset = _plain(table["toolset"])
        if set(toolset) != {"trait", "suffix", "members"}:
            raise ParityError("[rules] toolset is { trait = name, suffix = name, members = [...] }")
        for name in ("builtins", "builtin_errors"):
            for path in _plain(table.get(name, {})):
                if "::" not in path:
                    raise ParityError(f"[rules] {name}: {path!r} is no Rust path")
        packages = _plain(table["packages"])
        return cls(
            acronyms=acronyms,
            packages={crate: list(modules) for crate, modules in packages.items()},
            options_suffixes=tuple(table["options_suffixes"]),
            input_suffix=table["input_suffix"],
            arguments=table["arguments"],
            cancellation=table["cancellation"],
            factory=table["factory"],
            builder_prefix=table["builder_prefix"],
            toolset=Toolset(toolset["trait"], toolset["suffix"], tuple(toolset["members"])),
            builtins=_plain(table.get("builtins", {})),
            builtin_errors=_plain(table.get("builtin_errors", {})),
            decisions=_decisions(table),
            tables=_table_rules(table),
        )

    def setting(self, rule: str, name: str) -> str | None:
        """A table rule's setting, or None when the mapping has not the rule."""
        return self.tables.get(rule, {}).get(name)

    def python_type_name(self, rust: str) -> str:
        """A Rust type's name with its acronyms in Python's casing."""
        left, out = tokens(rust), []
        while left:
            for words, python in self.acronyms:
                if left[: len(words)] == words:
                    out.append(python)
                    left = left[len(words) :]
                    break
            else:
                out.append(left.pop(0))
        return "".join(out)

    def modules_for(self, path: str) -> list[str]:
        """The Python modules a Rust crate's items are looked for in."""
        crate = path.split("::")[0]
        modules = self.packages.get(crate)
        if modules is None:
            raise ParityError(f"[rules.packages] lists no Python module for {crate}")
        return modules


@dataclass
class Entry:
    table: str
    index: int
    rust: str | None = None
    python: str | None = None
    covers: list[str] = field(default_factory=list)
    params: dict[str, str] = field(default_factory=dict)
    reason: str = ""
    approved: str | None = None
    decision: str | None = None
    covered: list = field(default_factory=list)

    @property
    def name(self) -> str:
        return f"[[{self.table}]] #{self.index + 1} ({self.rust or self.python or self.covers[0]})"

    @property
    def pairs(self) -> bool:
        """The entry names a Rust and a Python counterpart."""
        return self.rust is not None and self.python is not None


class Mapping:
    def __init__(self, rules: Rules, entries: list[Entry]) -> None:
        self.rules = rules
        self.entries = entries

    @classmethod
    def from_table(cls, table: dict, rules_only: bool = False) -> Mapping:
        unknown = set(table) - {"rules", *_KEYS}
        if unknown or "rules" not in table:
            raise ParityError(
                f"the mapping holds [rules] and {sorted(_KEYS)}, "
                f"not {sorted(unknown) or 'no [rules]'}"
            )
        entries = [
            entry(kind, index, raw)
            for kind in _KEYS
            for index, raw in enumerate(table.get(kind, []))
        ]
        return cls(Rules.from_table(table["rules"]), [] if rules_only else entries)

    def of(self, table: str) -> list[Entry]:
        return [entry for entry in self.entries if entry.table == table]

    def pairings(self) -> list[Entry]:
        """The entries naming both counterparts: an approved `[[map]]`, or a
        `[[pending]]` whose name difference awaits a decision."""
        return [entry for entry in self.entries if entry.pairs]


def entry(table: str, index: int, raw: dict) -> Entry:
    """One entry, checked against its table's keys."""
    required, optional = _KEYS[table]
    where = f"[[{table}]] #{index + 1}"
    if missing := required - set(raw):
        raise ParityError(f"{where}: {', '.join(sorted(missing))} missing")
    if unknown := set(raw) - required - optional:
        raise ParityError(f"{where}: unknown {', '.join(sorted(unknown))}")
    if "approved" in raw and not _DATE.fullmatch(str(raw["approved"])):
        raise ParityError(f"{where}: approved is a date, YYYY-MM-DD")
    if ("rust" in raw) != ("python" in raw):
        raise ParityError(f"{where}: names both counterparts, rust and python, or neither")
    covers = list(raw.get("covers", []))
    for key in covers:
        found = _KEY.fullmatch(key)
        if not found or found.group(3) not in KINDS:
            raise ParityError(
                f"{where}: {key!r} is no finding key, `<rust|python> <path> (<kind>)`"
            )
    if table != "map" and not covers:
        raise ParityError(f"{where}: covers no finding")
    if table == "missing" and (
        kinds := sorted({parse_key(key)[2] for key in covers} - _MISSING_KINDS)
    ):
        raise ParityError(f"{where}: covers {kinds}, not a feature on one side only")
    decision = raw.get("decision")
    if decision is not None and (not isinstance(decision, str) or not decision.strip()):
        raise ParityError(f"{where}: decision names its source")
    params = raw.get("params", {})
    if not isinstance(params, dict):
        raise ParityError(f"{where}: params is a table of Rust names to Python names")
    return Entry(
        table=table,
        index=index,
        rust=raw.get("rust"),
        python=raw.get("python"),
        covers=covers,
        params=dict(params),
        reason=str(raw["reason"]),
        approved=str(raw["approved"]) if "approved" in raw else None,
        decision=decision,
    )


def _plain(rule: dict) -> dict:
    """A rule table without its `decision`."""
    return {key: value for key, value in rule.items() if key != "decision"}


def _table_rules(table: dict) -> dict[str, dict[str, str]]:
    found = {}
    for name, settings in _TABLE_RULES.items():
        if name not in table:
            continue
        rule = table[name]
        if not isinstance(rule, dict) or set(_plain(rule)) != settings:
            raise ParityError(f"[rules] {name} is a table of {', '.join(sorted(settings))}")
        found[name] = {key: str(value) for key, value in _plain(rule).items()}
    return found


def _decisions(table: dict) -> dict[str, str]:
    """Each rule table's `decision`: where the maintainer decided the rule."""
    found = {}
    for name, rule in table.items():
        if not isinstance(rule, dict) or "decision" not in rule:
            continue
        decision = rule["decision"]
        if not isinstance(decision, str) or not decision.strip():
            raise ParityError(f"[rules] {name}: decision names its source")
        found[name] = decision
    return found


def parse_key(key: str) -> tuple[str, str, str]:
    """A finding key as (side, path, kind)."""
    found = _KEY.fullmatch(key)
    if found is None:
        raise ParityError(f"{key!r} is no finding key")
    return found.group(1), found.group(2), found.group(3)


def load_mapping(path: Path, rules_only: bool) -> Mapping:
    try:
        table = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise ParityError(f"{path}: {error}") from error
    return Mapping.from_table(table, rules_only)
