#!/usr/bin/env python3
"""The public API parity gate: the Rust crates against the Python packages.

The rule (CONTRIBUTING.md, "One public API"): the Rust crates and the Python
packages have one public API, and Python is the reference. Only what the
language forces may differ; each such mapping is a rule below or an entry of
`api/mapping.toml`, and any other difference needs a maintainer's decision
before it is written. This script reads the Rust surface `scripts/rs-surface.py`
wrote, the Python snapshot `tests/python/api_signatures.py` wrote, and the
mapping, and fails on any difference no entry covers.

The rules, which apply with no entry, each forced by the language:
- names: a Rust type's name is Python's with an acronym in Rust's casing
  (`[rules] acronyms`); a function or a constant keeps its name; a Rust
  item is looked for in the Python modules its crate lists in
  `[rules.packages]`;
- receivers, cancellation, results: `&self` and `self` are no parameter, nor
  is a `CancellationToken` (`[rules] cancellation`): Python cancels the task
  that awaits the call; a `Result` stands for a raised exception and an
  `Output<T>` for the `{"data", "text"}` dict a call returns, the snapshot
  holding no Python annotation to compare a type with;
- async: Python's `async def` is an `async fn`, or a function returning a
  future, a trait object's boxed future alias (`*Future`) included, which
  has no Python name of its own; async on one side only is a finding;
- constructors: Python's constructor is an async factory (`[rules]
  factory`, `create`) when the class has one taking more than `**keywords`,
  since Python cannot await in `__init__`, whose `__init__` then wraps the
  native object the factory built and is not compared; else `__init__`.
  Rust's is `new` and the builders (`[rules] builder_prefix`, `with_<x>(self,
  value)`), a builder standing for the keyword of its parameter's name, or
  for its struct's fields; a struct with public fields and no `new` is
  built from its fields, as a dataclass;
- keywords as a struct of named fields: a Rust parameter whose type is an
  `<X>Input` (`[rules] input_suffix`) or an options struct (a name ending
  with one of `[rules] options_suffixes`) stands for its fields, each the
  Python parameter of its name. A field its struct's `new` takes is
  required; any other is defaulted, set by struct update, which needs `new`
  or `Default` (a struct with neither is built by a literal setting every
  field). A Python required parameter is a required Rust one, a defaulted
  one a defaulted one, whatever its position: a required keyword-only
  parameter is a positional Rust one. Defaults' values are not compared. A
  struct a caller builds keeps no `#[non_exhaustive]` (roomkit-rs convention
  9): that refuses struct update outside its crate;
- optional arguments: Rust has none, so an `Option<T>` parameter stands for
  Python's `=None` (`[rules] optional`), and for no other default;
- a dict for a struct: Python's `arguments` (`[rules] arguments`), the
  schema-shaped dict a call takes, is the Rust `<X>Input` parameter whole;
- contract types: a Rust type deriving `JsonSchema` is the JSON a Python call
  passes or returns as a dict, and has no Python class. Its fields are the
  properties of the JSON type of its name in the Python snapshot's tool
  contracts, its unit variants the values, its variants with fields the
  tagged variants (see `api_parity/wire.py`);
- properties: a Python property is a Rust field, or a method of `&self`
  alone;
- traits: a Rust trait is the Python class (a protocol) of its name; a type
  implementing a trait of the surface has its methods, and a Python member a
  subclass repeats from its base unchanged is compared on the base. A trait
  against a protocol compares no default: its implementation receives every
  argument;
- errors: an error enum (one implementing `Error`) is its Python base
  class, each variant `X` the class `<Stem>XError` or `XError` of the
  classes deriving from the base (`<Stem>` the base's name without
  `Error`), in Python's acronym casing, or the builtin exception `[rules]
  builtin_errors` names; a class that takes a message alone has its
  variant's payload compose it;
- builtins: a Rust type `[rules] builtins` names stands for a Python builtin
  (`Output` and `Definition` are the dicts a call returns); a member of it
  is a finding;
- the RoomKit tool protocol: Python's RoomKit tool members (`definition`,
  `handler`) are the engine's `Toolset` implementation (`[rules] toolset`);
  a struct implementing it, named with the toolset suffix, is compared with
  the Python RoomKit tool named without it, the suffix itself a name
  finding;
- a rule naming a Rust item or variant that is gone, or that no comparison
  reads, is a finding no entry covers.

How a difference is reported, each finding of a kind (`mapping.KINDS`)
and none silenced:
- parameters are matched by name; of what is left, a Python `**name` against
  the fields of the struct a Rust parameter or builder `name` brings is one
  `variadic` finding, a Rust field holding an options struct whose fields
  Python takes flat one `form` finding, and a single parameter left on each
  side one `name` finding;
- a Rust constructor taking a built tool, where Python takes that tool's
  constructor parameters, is one `form` finding; a constructor returning
  another type than its own is a `result` finding, and so is a tool call
  taking or returning another type than its contract's, for a Python class
  serving one contract;
- a contract type named as a Python class is a `meaning` finding, and one no
  tool contract holds an `uncompared` one.

What a rule does not give, an entry of `api/mapping.toml` does: `[[map]]`
names the Python counterpart of a Rust item or member, a mapping the
maintainer approved as forced by the language; `[[exception]]` covers
approved one-sided findings; `[[missing]]` covers a feature one side has
and the other lacks, recorded rather than ported (`unmatched` and
`uncompared` findings only); `[[pending]]` covers the findings awaiting a
decision, and, naming both counterparts, pairs two items or members whose
name difference awaits it. An entry covers findings by key,
`<rust|python> <path> (<kind>)`; a key no finding has, a pairing the rules
already make, or an entry naming what is gone, is dead. An entry, and a
rule written as a table, names in `decision` where the maintainer decided
it.

The gate fails (exit 1) on a finding no entry covers or a dead entry;
`--strict` also on any `[[pending]]` entry left. An input it cannot read
whole exits 2.

Usage: python3 scripts/api-parity.py SURFACE... [--python DIR]... [--mapping FILE]
                                     [--rules-only] [--strict] [--verbose]
                                     [--json OUT] [--pending]
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import TextIO

# The package beside this script holds the gate's parts.
sys.path.insert(0, str(Path(__file__).resolve().parent))

from api_parity import ParityError  # noqa: E402
from api_parity.cover import Report, compare  # noqa: E402
from api_parity.mapping import Mapping, load_mapping  # noqa: E402
from api_parity.report import as_json, pending_toml, render  # noqa: E402
from api_parity.sides import Python, Surface, load_python, load_surface  # noqa: E402

__all__ = ["Mapping", "ParityError", "Python", "Report", "Surface", "compare", "main"]

PYTHON = [Path("tests/python"), Path("backends/rusto/python/tests")]


def main(argv: list[str] | None = None, out: TextIO = sys.stdout, log: TextIO = sys.stderr) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("surfaces", type=Path, nargs="+", help="the JSON rs-surface.py wrote")
    parser.add_argument(
        "--python", type=Path, action="append", help="a Python snapshot's directory"
    )
    parser.add_argument("--mapping", type=Path, default=Path("api/mapping.toml"))
    parser.add_argument(
        "--rules-only", action="store_true", help="apply the rules, none of the entries"
    )
    parser.add_argument(
        "--strict", action="store_true", help="also fail while a [[pending]] entry is left"
    )
    parser.add_argument("--verbose", action="store_true", help="also list the pending findings")
    parser.add_argument("--json", type=Path, help="also write the findings as JSON there")
    parser.add_argument(
        "--pending",
        action="store_true",
        help="print [[pending]] entries for the findings without one",
    )
    args = parser.parse_args(argv)
    try:
        python = load_python(args.python or PYTHON)
        report = compare(
            load_surface(args.surfaces), python, load_mapping(args.mapping, args.rules_only)
        )
    except ParityError as error:
        print(f"api-parity: {error}", file=log)
        return 2
    if args.pending:
        out.write(pending_toml(report.uncovered))
        return 0
    out.write(render(report, strict=args.strict, verbose=args.verbose))
    if args.json:
        args.json.write_text(
            json.dumps(as_json(report), indent=1, ensure_ascii=False) + "\n", encoding="utf-8"
        )
    return 0 if report.passed(args.strict) else 1


if __name__ == "__main__":
    sys.exit(main())
