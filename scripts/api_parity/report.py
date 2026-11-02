"""The report, as text, as JSON, and as the `[[pending]]` entries it lacks."""

from __future__ import annotations

import json

from api_parity.compare import Finding
from api_parity.cover import Report
from api_parity.mapping import UNDECIDED


def render(report: Report, *, strict: bool, verbose: bool) -> str:
    lines: list[str] = []
    out = lines.append
    tables = ("map", "exception", "missing", "pending")
    counts = ", ".join(f"{len(report.mapping.of(t))} [[{t}]]" for t in tables)
    rustdoc = ", ".join(version for version in report.surface.rustdoc if version) or "unknown"
    out(f"API parity: Rust crates (rustdoc {rustdoc}) against the Python snapshot")
    out(f"Rust: {len(report.surface.items)} items. Python: {len(report.python.names())} names.")
    out(f"Mapping: {counts}.")
    pending, missing = len(report.pending), len(report.missing)
    excepted = len(report.covered_by("exception"))
    out(
        f"Findings: {len(report.findings)}; {pending} pending, {excepted} excepted, "
        f"{missing} on one side only, {len(report.uncovered)} without an entry."
    )
    out("")
    _findings(out, "Findings without an entry", report.uncovered)
    out(f"== Dead entries: {len(report.dead)}")
    for dead in report.dead:
        out(f"  {dead.entry}: {dead.path}: {dead.why}")
    out("")
    undecided = len(report.undecided)
    out(f"== Awaiting the maintainer's decision (decision = {UNDECIDED!r}): {undecided}")
    for where in report.undecided:
        out(f"  {where}")
    out("")
    entries = report.mapping.of("pending")
    out(f"== Pending, awaiting a maintainer's decision: {len(entries)} entries, {pending} findings")
    if verbose:
        _findings(out, "Pending findings", report.pending)
    else:
        out("  (--verbose lists them)")
        out("")
    passed = report.passed(strict)
    verdict = "passed" if passed else "failed"
    why = []
    if report.uncovered:
        why.append(f"{len(report.uncovered)} findings without an entry")
    if report.dead:
        why.append(f"{len(report.dead)} dead entries")
    if report.undecided:
        why.append(f"{len(report.undecided)} awaiting the maintainer's decision")
    if strict and entries:
        why.append(f"{len(entries)} pending entries (--strict)")
    out(f"Gate: {verdict}" + (f": {'; '.join(why)}." if why else "."))
    return "\n".join(lines) + "\n"


def _findings(out, title: str, findings: list[Finding]) -> None:
    out(f"== {title}: {len(findings)}")
    for (rust, python), found in _by_item(findings).items():
        out(f"  {rust or '(no Rust item)'} / {python or '(no Python name)'}")
        for finding in found:
            out(f"    {finding.side:<6} {finding.path} ({finding.kind}): {finding.detail}")
    out("")


def _by_item(findings: list[Finding]) -> dict[tuple[str | None, str | None], list[Finding]]:
    grouped: dict[tuple[str | None, str | None], list[Finding]] = {}
    for finding in findings:
        grouped.setdefault((finding.rust_item, finding.python_item), []).append(finding)
    return grouped


def pending_toml(findings: list[Finding]) -> str:
    """`[[pending]]` entries covering `findings`, one per Rust item and
    Python name, each key's detail as a comment."""
    blocks = []
    grouped = _by_item(findings)
    for rust, python in sorted(grouped, key=lambda pair: (pair[0] or "~", pair[1] or "")):
        found = grouped[(rust, python)]
        lines = [
            "[[pending]]",
            f"reason = {json.dumps(_reason(rust, python, found))}",
            "covers = [",
        ]
        lines += [f"  {json.dumps(f.key)},  # {f.detail}" for f in found]
        lines.append("]")
        blocks.append("\n".join(lines))
    return "\n\n".join(blocks) + "\n"


def _reason(rust: str | None, python: str | None, found: list[Finding]) -> str:
    """The pair of counterparts, and the kinds of what differs (KINDS)."""
    kinds: dict[str, int] = {}
    for finding in found:
        kinds[finding.kind] = kinds.get(finding.kind, 0) + 1
    what = ", ".join(kind if count == 1 else f"{kind} ({count})" for kind, count in kinds.items())
    return f"{rust or 'no Rust item'} / {python or 'no Python name'}: {what}"


def as_json(report: Report) -> dict:
    def finding(f: Finding) -> dict:
        return {
            "side": f.side,
            "path": f.path,
            "kind": f.kind,
            "detail": f.detail,
            "rust_item": f.rust_item,
            "python_item": f.python_item,
            "key": f.key,
        }

    return {
        "findings": [finding(f) for f in report.findings],
        "uncovered": [finding(f) for f in report.uncovered],
        "dead": [{"entry": d.entry, "path": d.path, "why": d.why} for d in report.dead],
        "undecided": report.undecided,
        **{
            table: [
                {"entry": e.name, "reason": e.reason, "covered": [finding(f) for f in e.covered]}
                for e in report.mapping.of(table)
            ]
            for table in ("missing", "pending")
        },
    }
