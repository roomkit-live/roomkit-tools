"""The ends of background jobs, as a host reads them with ``async for``."""

from __future__ import annotations

import json
from typing import Any

from . import _native


class _JobEnds:
    """The ends a native subscription tells, each a dict, until it stops.

    The subscription is taken when the session's ends() is called, not at
    the first iteration: an end between the two is not lost.
    """

    def __init__(self, native: _native._JobEnds) -> None:
        self._native = native

    def __aiter__(self) -> _JobEnds:
        return self

    async def __anext__(self) -> dict[str, Any]:
        end = await self._native.next()
        if end is None:
            raise StopAsyncIteration
        return json.loads(end)
