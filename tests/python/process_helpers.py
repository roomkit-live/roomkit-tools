"""Observe Linux fixture processes without relying on arbitrary start delays."""

import asyncio
import os
import signal
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


def running(pid: int) -> bool:
    """Whether pid runs, neither gone nor a zombie."""
    try:
        stat = Path(f"/proc/{pid}/stat").read_text()
    except FileNotFoundError:
        return False
    return stat.rsplit(") ", 1)[1][0] not in {"Z", "X"}


def kill(ids: list[int]) -> None:
    """Stop fixture processes the code under test left running."""
    for pid in ids:
        try:
            os.kill(pid, signal.SIGKILL)
        except ProcessLookupError:
            pass


COMMAND_WITH_CHILD = "sleep 60 & printf '%s %s' $$ $! > pids; wait"
