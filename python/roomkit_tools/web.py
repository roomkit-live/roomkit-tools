"""Independent, asynchronous HTTP fetching implemented in Rust."""

from __future__ import annotations

import json
from typing import Any, Self

from . import _native


class WebFetchTool:
    """HTTP(S) GET with host policy, UTF-8 content and bounded conversion.

    No I/O at construction. Public destinations are permitted by default.
    allowed_origins optionally restricts exact scheme/host/port origins;
    private_origins explicitly permits local/intranet addresses for those
    origins without bypassing allowed_origins. Empty allowed_origins denies all.
    Redirects repeat access checks and DNS addresses are pinned per connection.
    Inherited HTTP proxies, cookies and URL credentials are not used.

    Defaults: 8 KiB URL, 5 MiB decoded HTTP body, 5 MiB converted text,
    50 KiB returned content, 10 redirects, 30 s default / 120 s maximum deadline.
    HTML conversion admits at most 100,000 nodes and 128 levels. The converted
    bound is checked on every write of the converted text; it is not a hard
    peak-memory limit. Two blocking conversion slots are shared by calls to this
    instance. Cancellation/deadline drops network work and stops conversion
    within a few hundred nodes; issued DNS may finish later.
    """

    def __init__(
        self,
        *,
        allowed_origins: list[str] | None = None,
        private_origins: list[str] | None = None,
        max_url_bytes: int | None = None,
        max_response_bytes: int | None = None,
        max_converted_bytes: int | None = None,
        max_output_bytes: int | None = None,
        max_redirects: int | None = None,
        default_timeout_ms: int | None = None,
        max_timeout_ms: int | None = None,
    ) -> None:
        limits = {
            key: value
            for key, value in {
                "max_url_bytes": max_url_bytes,
                "max_response_bytes": max_response_bytes,
                "max_converted_bytes": max_converted_bytes,
                "max_output_bytes": max_output_bytes,
                "max_redirects": max_redirects,
                "default_timeout_ms": default_timeout_ms,
                "max_timeout_ms": max_timeout_ms,
            }.items()
            if value is not None
        }
        policy = {
            "allowed_origins": allowed_origins,
            "private_origins": [] if private_origins is None else private_origins,
        }
        self._native = _native._WebFetchTool(
            json.dumps(limits, allow_nan=False), json.dumps(policy)
        )

    @classmethod
    async def create(cls, **configuration: Any) -> Self:
        """Construct without network access; also available through the constructor."""
        return cls(**configuration)

    @staticmethod
    def definition() -> dict[str, Any]:
        """Return the Rust-derived model schemas; host policy is never model input."""
        return json.loads(_native._WebFetchTool.definition_json())

    async def call(self, arguments: dict[str, Any]) -> dict[str, Any]:
        """Fetch Rust-validated input and return data/text, including HTTP errors.

        A non-2xx status has http_ok=False. Unsupported encodings/content and
        transport failures raise typed errors. Check truncated before claiming
        completeness. No archive, cursor or browser execution is provided.
        """
        token = _native._Cancellation()
        try:
            return json.loads(
                await self._native.call(json.dumps(arguments, allow_nan=False), token)
            )
        finally:
            token.cancel()

    async def fetch(
        self, url: str, *, format: str = "markdown", timeout_ms: int | None = None
    ) -> dict[str, Any]:
        """Fetch as markdown, text or html; content must be UTF-8/ASCII text."""
        return await self.call({"url": url, "format": format, "timeout_ms": timeout_ms})
