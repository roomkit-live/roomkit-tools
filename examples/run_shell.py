"""Run foreground shell commands through Rust, without RoomKit or a model."""

import asyncio
import tempfile

from roomkit_tools import ShellTool, Workspace, WriteTool


async def main() -> None:
    with tempfile.TemporaryDirectory(prefix="roomkit-shell-") as directory:
        workspace = Workspace(directory)
        write = await WriteTool.create(workspace)
        shell = await ShellTool.create(workspace)
        await write.write("notes.txt", "Status: ready\n")
        await write.write("summary.txt", "Count: 3\n")
        result = await shell.shell("cat notes.txt summary.txt")
        assert result["data"]["exit_code"] == 0
        assert result["data"]["stdout"]["text"] == "Status: ready\nCount: 3\n"
        print(result["text"])

        result = await shell.shell("printf warning >&2; exit 7")
        assert result["data"]["exit_code"] == 7
        assert result["data"]["stderr"]["text"] == "warning"
        print(result["text"])

        result = await shell.shell("printf started; sleep 5", timeout_ms=200)
        assert result["data"]["timed_out"]
        assert result["data"]["stdout"]["text"] == "started"
        assert result["data"]["signal"] == 9
        print(result["text"])


if __name__ == "__main__":
    asyncio.run(main())
