"""The public API parity gate between the Rust crates and the Python packages.

`scripts/api-parity.py` is the command; this package holds its parts:
- `sides`: the Rust surface `scripts/rs-surface.py` writes, and the Python
  snapshot `tests/python/api_signatures.py` writes;
- `mapping`: `api/mapping.toml`, its rules and its entries;
- `rust`: what a Rust method's parameters stand for in Python;
- `compare`: the pairing of each Python name with the Rust items standing
  for it, and a function or a method against theirs;
- `members`: a class's members against theirs, the traits they implement
  included;
- `constructors`, `params`, `errors`: a constructor, a call's parameters and
  an error enum against Python's;
- `defaults`: Python's default values against the Rust dump's, and
  `values` the literals both sides write;
- `calls`: Python's `call` and `definition` against the typed methods they
  stand for;
- `wire`: the contract types against the JSON the Python calls exchange;
- `leftovers`: what no Python name stands for, and back;
- `cover`: what the entries cover, and the dead ones;
- `report`: the report, as text, as JSON, and as `[[pending]]` entries.
"""


class ParityError(Exception):
    """An input the gate cannot read whole."""
