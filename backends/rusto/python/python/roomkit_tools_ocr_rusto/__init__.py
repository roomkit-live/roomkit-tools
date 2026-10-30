"""Optional RustO model backend; all OCR execution and bounds remain in Rust."""

from __future__ import annotations

import json
import os
from pathlib import Path
from typing import Any

from . import _native


class _Bound:
    def __init__(self, native: Any) -> None:
        self._native = native

    async def call(self, arguments: dict[str, Any]) -> dict[str, Any]:
        token = _native.Cancellation()
        try:
            return json.loads(
                await self._native.call(json.dumps(arguments, allow_nan=False), token)
            )
        finally:
            token.cancel()


class RustoOcr:
    """Own one lazily loaded PP-OCRv6 model pair with private worker pools.

    models contains det.onnx, rec.onnx and dict.txt. Files must be trusted and
    unchanged during loading. No downloads or global environment changes occur.
    Calls across all tools bound to this backend share one native execution slot.
    """

    def __init__(
        self,
        models: str | os.PathLike[str],
        *,
        model_name: str = "ppocrv6",
        threads: int = 2,
        detection_max_edge: int = 960,
    ) -> None:
        root = Path(models).absolute()
        self._engine = _native.Engine(
            json.dumps(
                {
                    "detection_model": str(root / "det.onnx"),
                    "recognition_model": str(root / "rec.onnx"),
                    "dictionary": str(root / "dict.txt"),
                    "model_name": model_name,
                    "threads": threads,
                    "detection_max_edge": detection_max_edge,
                },
                allow_nan=False,
            )
        )

    async def bind(self, root: str, limits: dict[str, int]) -> _Bound:
        """Bind a workspace without duplicating models; omitted limits use Rust defaults."""
        token = _native.Cancellation()
        try:
            return _Bound(await self._engine.bind(root, json.dumps(limits, allow_nan=False), token))
        finally:
            token.cancel()


__all__ = ["RustoOcr"]
