"""List, discover, search and read files through Rust, without RoomKit or a model.

After installing roomkit-tools: python examples/discover_files.py
"""

import asyncio
import tempfile
from pathlib import Path

from roomkit_tools import GlobTool, GrepTool, ListTool, ReadTool


async def main(root: Path) -> None:
    listing = await ListTool.create(root)
    glob = await GlobTool.create(root)
    grep = await GrepTool.create(root)
    read = await ReadTool.create(root)

    directories = await listing.list()
    paths = await glob.glob("**/*.txt")
    matches = await grep.grep("blue", include="**/*.txt", context_before=2)
    source = await read.read(matches["data"]["matches"][0]["path"])

    for name, result in [
        ("list", directories),
        ("glob", paths),
        ("grep", matches),
        ("read", source),
    ]:
        print(f"{name}:\n{result['text']}")

    assert [entry["path"] for entry in paths["data"]["entries"]] == ["catalog/widgets.txt"]
    assert matches["data"]["matches"][0]["context_before"][0]["text"] == "Widget A"
    assert source["text"] == "1: Widget A\n2: Price: 12 EUR\n3: Color: blue\n"


if __name__ == "__main__":
    with tempfile.TemporaryDirectory(prefix="roomkit-discovery-") as directory:
        root = Path(directory)
        (root / "catalog").mkdir()
        (root / "catalog/widgets.txt").write_text(
            "Widget A\nPrice: 12 EUR\nColor: blue\n", encoding="utf-8"
        )
        asyncio.run(main(root))
