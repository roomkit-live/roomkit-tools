"""What the entries cover: each finding's entry, and the dead entries."""

from __future__ import annotations

from dataclasses import dataclass

from api_parity.compare import Comparison, Finding
from api_parity.mapping import Mapping, parse_key
from api_parity.sides import Python, Surface


@dataclass(frozen=True)
class Dead:
    entry: str
    path: str
    why: str


@dataclass
class Report:
    surface: Surface
    python: Python
    mapping: Mapping
    findings: list[Finding]
    uncovered: list[Finding]
    dead: list[Dead]

    @property
    def pending(self) -> list[Finding]:
        return self.covered_by("pending")

    @property
    def missing(self) -> list[Finding]:
        """The features one side lacks, recorded, not ported."""
        return self.covered_by("missing")

    @property
    def undecided(self) -> list[str]:
        """The rules and entries whose decision is left to the maintainer."""
        return self.mapping.undecided()

    def covered_by(self, table: str) -> list[Finding]:
        return [finding for entry in self.mapping.of(table) for finding in entry.covered]

    def passed(self, strict: bool) -> bool:
        """No finding without an entry, no dead entry, no decision left to
        the maintainer; `strict`, no pending entry either."""
        return (
            not self.uncovered
            and not self.dead
            and not self.undecided
            and not (strict and self.mapping.of("pending"))
        )


def compare(surface: Surface, python: Python, mapping: Mapping) -> Report:
    comparison = Comparison(surface, python, mapping)
    findings = comparison.run()
    uncovered = _cover(findings, mapping)
    dead = _dead(comparison, findings, mapping)
    return Report(surface, python, mapping, findings, uncovered, dead)


def _cover(findings: list[Finding], mapping: Mapping) -> list[Finding]:
    """Gives each finding to the entry naming its key, and returns those no
    entry names. No entry covers a finding about the rules themselves: the
    rules are fixed, not excused."""
    by_key = {key: entry for entry in mapping.entries for key in entry.covers}
    uncovered = []
    for finding in findings:
        entry = by_key.get(finding.key)
        if entry is None or finding.kind == "rules":
            uncovered.append(finding)
        else:
            entry.covered.append(finding)
    return uncovered


def _dead(comparison: Comparison, findings: list[Finding], mapping: Mapping) -> list[Dead]:
    """An entry naming what is gone, a pairing the rules already make or
    that applies nowhere, and a key no finding has."""
    surface, python, rules = comparison.surface, comparison.python, comparison.rules
    keys = {finding.key for finding in findings}
    dead = []
    seen: dict[str, str] = {}
    for entry in mapping.entries:
        if entry.pairs:
            gone = [
                (side, path)
                for side, path in (("rust", entry.rust), ("python", entry.python))
                if not (surface if side == "rust" else python).exists(path)
            ]
            dead += [
                Dead(entry.name, path, f"names nothing of the {side} side") for side, path in gone
            ]
            rust_name = entry.rust.rpartition("::")[2]
            if not gone and rules.python_type_name(rust_name) == entry.python.rpartition(".")[2]:
                dead.append(
                    Dead(
                        entry.name, entry.rust, "pairs what the rules pair: the names are the same"
                    )
                )
            elif not gone and id(entry) not in comparison.applied:
                dead.append(
                    Dead(
                        entry.name,
                        entry.rust,
                        f"applies nowhere: {entry.python} is compared with another Rust item",
                    )
                )
        for key in entry.covers:
            if key in seen:
                dead.append(Dead(entry.name, key, f"also covered by {seen[key]}"))
            seen[key] = entry.name
            if key not in keys:
                side, path, _ = parse_key(key)
                exists = (surface if side == "rust" else python).exists(path)
                why = (
                    "covers nothing: no such finding"
                    if exists
                    else f"names nothing of the {side} side"
                )
                dead.append(Dead(entry.name, key, why))
    return dead
