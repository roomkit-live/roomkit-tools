"""Compare files, preview a batch, then apply it using the Rust Python tools."""

import asyncio
import tempfile
from pathlib import Path

from roomkit_tools import DiffTool, PatchTool, Workspace, WriteTool


async def main() -> None:
    with tempfile.TemporaryDirectory(prefix="roomkit-patch-") as directory:
        workspace = Workspace(directory)
        write = await WriteTool.create(workspace)
        diff = await DiffTool.create(workspace)
        patch = await PatchTool.create(workspace)
        before, after = "count=2\n", "count=3\n"
        await write.write("current.txt", before)
        await write.write("proposed.txt", after)
        comparison = await diff.diff("current.txt", "proposed.txt")
        assert comparison["data"]["changed"]
        print(comparison["text"])

        changes = [
            {"op": "update", "path": "current.txt", "old_content": before, "new_content": after},
            {"op": "add", "path": "summary.txt", "content": "Count increased.\n"},
        ]
        preview = await patch.patch(changes)
        assert preview["data"]["dry_run"] and preview["data"]["applied"] == []
        assert await asyncio.to_thread(Path(directory, "current.txt").read_text) == before
        assert not await asyncio.to_thread(Path(directory, "summary.txt").exists)
        print(preview["text"])

        result = await patch.patch(changes, dry_run=False)
        assert result["data"]["completed"] and result["data"]["failure"] is None
        assert len(result["data"]["applied"]) == 2
        assert await asyncio.to_thread(Path(directory, "current.txt").read_text) == after
        assert (
            await asyncio.to_thread(Path(directory, "summary.txt").read_text)
            == "Count increased.\n"
        )
        assert not (await diff.diff("current.txt", "proposed.txt"))["data"]["changed"]
        print(result["text"])


if __name__ == "__main__":
    asyncio.run(main())
