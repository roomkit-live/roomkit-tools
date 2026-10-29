"""Observe Linux fixture processes without relying on arbitrary start delays."""

import asyncio
from pathlib import Path


async def read_pids(path: Path) -> list[int]:
    async with asyncio.timeout(5):
        while True:
            try:
                values = (await asyncio.to_thread(path.read_text)).split()
                if len(values) == 2:
                    return [int(value) for value in values]
            except (FileNotFoundError, ValueError):
                pass
            await asyncio.sleep(0.005)


async def assert_stopped(ids: list[int]) -> None:
    """Require the shell to be reaped and its child to be dead or an init-owned zombie."""
    async with asyncio.timeout(3):
        while True:
            alive = False
            for pid in ids:
                try:
                    stat = await asyncio.to_thread(Path(f"/proc/{pid}/stat").read_text)
                    state = stat.rsplit(") ", 1)[1][0]
                    alive |= state not in {"Z", "X"}
                except (FileNotFoundError, ProcessLookupError):
                    pass
            if not alive and not await asyncio.to_thread(Path(f"/proc/{ids[0]}").exists):
                return
            await asyncio.sleep(0.005)


COMMAND_WITH_CHILD = "sleep 60 & printf '%s %s' $$ $! > pids; wait"
