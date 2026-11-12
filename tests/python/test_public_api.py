"""The installed package's public API against its committed snapshot.

The snapshot is the reference the Rust crates are compared with
(``make api-parity``), so any change to the public Python surface, a
compatible addition included, fails here until ``make api-snapshot``
rewrites it in the same commit.
"""

import json
import shutil
import tempfile
import unittest
from pathlib import Path

import api_signatures
from api_signatures import Package

HERE = Path(__file__).resolve().parent


class SnapshotTests(unittest.TestCase):
    def test_the_snapshot_holds_the_whole_public_surface(self):
        drift = api_signatures.drift(HERE, Package("roomkit_tools"), with_contracts=True)
        self.assertEqual(drift, [], "run make api-snapshot and commit the rewritten files")

    def test_a_snapshot_that_differs_from_the_code_is_drift(self):
        """A name, a parameter or a contract property the snapshot lacks, or
        holds and the code does not, is drift."""
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            for name in (api_signatures.NAMES, api_signatures.SIGNATURES, api_signatures.CONTRACTS):
                shutil.copy(HERE / name, directory / name)
            self._edit(directory / api_signatures.NAMES, lambda names: names["roomkit_tools"].pop())
            self._edit(
                directory / api_signatures.SIGNATURES,
                lambda specs: specs["roomkit_tools.ReadTool"]["read"].append("extra=None"),
            )
            self._edit(
                directory / api_signatures.CONTRACTS,
                lambda contracts: contracts["types"]["ReadPage"]["fields"].pop(),
            )
            drift = api_signatures.drift(directory, Package("roomkit_tools"), with_contracts=True)
        self.assertEqual(len(drift), 3, drift)

    @staticmethod
    def _edit(path, change):
        held = json.loads(path.read_text(encoding="utf-8"))
        change(held)
        path.write_text(json.dumps(held), encoding="utf-8")


class DescribeTests(unittest.TestCase):
    """How the snapshot writes a class, on classes of a fake package."""

    package = Package(__name__)

    def test_a_protocol_has_no_constructor(self):
        from typing import Protocol

        class Backend(Protocol):
            async def read(self, *, offset: int) -> dict: ...

        Backend.__module__ = __name__
        self.assertEqual(
            api_signatures.describe(Backend, self.package, {}),
            {"read": ["async def", "*", "offset"]},
        )

    def test_an_async_factory_and_a_static_definition(self):
        class Tool:
            def __init__(self, native: object) -> None: ...

            @classmethod
            async def create(cls, root: str, *, max_bytes: int | None = None) -> "Tool": ...

            @staticmethod
            def definition() -> dict: ...

            @property
            def closed(self) -> bool: ...

        Tool.__module__ = __name__
        self.assertEqual(
            api_signatures.describe(Tool, self.package, {}),
            {
                "__init__": ["def", "native"],
                "closed": ["property"],
                "create": ["async classmethod", "root", "*", "max_bytes=None"],
                "definition": ["staticmethod"],
            },
        )

    def test_a_parameter_taking_a_native_object_is_marked(self):
        """An ``__init__`` parameter annotated with private native types only
        is the native object an async factory built: ``__native__`` names it."""

        class Tool:
            def __init__(self, native, root: str) -> None: ...

        class Other:
            def __init__(self, implementation, path) -> None: ...

        # Postponed annotations, as the package writes them.
        Tool.__init__.__annotations__["native"] = "_native._ReadTool | _native._ListTool"
        Other.__init__.__annotations__.update(
            implementation="BoundBackend", path="str | _native._X"
        )
        Tool.__module__ = Other.__module__ = __name__
        self.assertEqual(
            api_signatures.describe(Tool, self.package, {}),
            {"__init__": ["def", "native", "root"], "__native__": ["native", "native"]},
        )
        self.assertEqual(
            api_signatures.describe(Other, self.package, {}),
            {"__init__": ["def", "implementation", "path"]},
        )

    def test_a_public_base_of_the_package_is_written(self):
        class Base(Exception):
            pass

        class Child(Base):
            pass

        Base.__module__ = Child.__module__ = __name__
        self.assertEqual(
            api_signatures.describe(Child, self.package, {id(Base): "pkg.Base"}),
            {"__bases__": ["bases", "pkg.Base"]},
        )


class ShapeTests(unittest.TestCase):
    """A contract's JSON type as the snapshot keeps it."""

    def test_an_object_keeps_its_property_names(self):
        schema = {"type": "object", "properties": {"b": {}, "a": {}}, "required": ["a"]}
        self.assertEqual(api_signatures._shape(schema), {"fields": ["a", "b"]})

    def test_an_enumeration_keeps_its_values(self):
        self.assertEqual(api_signatures._shape({"enum": ["x", "y"]}), {"values": ["x", "y"]})
        constants = {"oneOf": [{"const": "x"}, {"const": "y", "description": "Y."}]}
        self.assertEqual(api_signatures._shape(constants), {"values": ["x", "y"]})

    def test_a_tagged_union_keeps_its_tags_and_their_properties(self):
        schema = {
            "oneOf": [
                {"properties": {"op": {"const": "add"}, "path": {}, "content": {}}},
                {"properties": {"op": {"const": "delete"}, "path": {}}},
            ]
        }
        self.assertEqual(
            api_signatures._shape(schema),
            {"tag": "op", "variants": {"add": ["content", "path"], "delete": ["path"]}},
        )

    def test_an_object_listing_its_variants_under_any_of_is_a_tagged_union(self):
        schema = {
            "type": "object",
            "anyOf": [
                {"properties": {"op": {"const": "add"}, "path": {}}},
                {"properties": {"op": {"const": "delete"}, "path": {}}},
            ],
        }
        self.assertEqual(
            api_signatures._shape(schema),
            {"tag": "op", "variants": {"add": ["path"], "delete": ["path"]}},
        )

    def test_a_type_it_cannot_read_is_refused(self):
        with self.assertRaises(ValueError):
            api_signatures._shape({"type": "string"})


if __name__ == "__main__":
    unittest.main()
