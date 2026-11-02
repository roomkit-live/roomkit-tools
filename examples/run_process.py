"""Use persistent stdin and byte cursors through Rust, without RoomKit or a model."""

import asyncio
import tempfile

from roomkit_tools import ProcessSession


async def main() -> None:
    with tempfile.TemporaryDirectory(prefix="roomkit-process-") as directory:
        async with ProcessSession(directory) as session:
            started = await session.start("cat")
            process_id = started["data"]["process_id"]
            sent = await session.write(process_id, "Hello\n")
            assert sent["data"]["complete"]
            assert sent["data"]["bytes_written"] == 6

            # Read only observes available output; it does not wait or send EOF.
            output = bytearray()
            offset = 0
            async with asyncio.timeout(5):
                while len(output) < 6:
                    result = await session.read(process_id, stdout_offset=offset, max_bytes=3)
                    chunk = result["data"]["stdout"]
                    assert chunk["omitted_bytes"] == 0
                    output.extend(chunk["bytes"])
                    offset = chunk["next_offset"]
                    await asyncio.sleep(0.01)
            assert output == b"Hello\n"
            assert result["data"]["status"]["state"] == "running"
            assert not result["data"]["status"]["stdin_closed"]
            print(output.decode(), end="")

            await session.write(process_id, close_stdin=True)
            async with asyncio.timeout(5):
                while True:
                    status = (await session.status(process_id))["data"]["processes"][0]
                    if status["state"] == "finished" and status["output_complete"]:
                        break
                    await asyncio.sleep(0.01)
            assert status["exit_code"] == 0
            assert status["signal"] is None
            assert (await session.stop(process_id, forget=True))["data"]["forgotten"]
            assert (await session.status())["data"]["processes"] == []


if __name__ == "__main__":
    asyncio.run(main())
