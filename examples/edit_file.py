"""Create, read and edit a temporary file through Rust, without RoomKit.

After installing roomkit-tools: python examples/edit_file.py
"""

import asyncio
import tempfile
from pathlib import Path

from roomkit_tools import AccessRefusedError, EditTool, ReadTool, Workspace, WriteTool


async def main(root: Path) -> None:
    workspace = Workspace(root)
    read = await ReadTool.create(workspace)
    write = await WriteTool.create(workspace)
    edit = await EditTool.create(workspace)

    created = await write.write("notes.txt", "Status: draft\n")
    before = await read.read("notes.txt")
    changed = await edit.edit(
        "notes.txt", "draft", "ready", expected_revision=before["data"]["revision"]
    )
    after = await read.read("notes.txt")
    assert after["text"] == "1: Status: ready\n"
    assert after["data"]["revision"] == changed["data"]["revision"]

    try:
        await edit.edit("notes.txt", "ready", "stale", expected_revision=before["data"]["revision"])
    except AccessRefusedError:
        print("Stale revision refused; file preserved.")
    else:
        raise AssertionError("A stale mutation must be refused")
    assert (await read.read("notes.txt"))["data"]["revision"] == after["data"]["revision"]
    print(created["text"], changed["text"], after["text"], sep="")


if __name__ == "__main__":
    with tempfile.TemporaryDirectory(prefix="roomkit-mutation-") as directory:
        asyncio.run(main(Path(directory)))
