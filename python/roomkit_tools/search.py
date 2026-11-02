"""Explicit, independently selectable search services, implemented in Rust.

The agent model is independent of this backend. Keys are never read implicitly
from environment variables. Exa supports limited anonymous searches; Ollama's
hosted API requires an account key, not a local Ollama installation.
"""

from __future__ import annotations

import json
from typing import Any, Self

from . import _native


class SearchBackend:
    """Base for packaged native backends; not a Python network implementation.

    Third-party Rust services implement the open SearchBackend trait. Exposing
    an additional Rust implementation to Python requires a binding factory.
    """

    _native: _native._SearchBackend

    @property
    def capabilities(self) -> dict[str, Any]:
        """Native provider result kinds and service limits, for host inspection."""
        return json.loads(self._native.capabilities_json())


def _config(
    endpoint: str | None,
    allowed_origins: list[str] | None,
    private_origins: list[str] | None,
    max_response_bytes: int,
) -> str:
    return json.dumps(
        {
            "endpoint": endpoint,
            "allowed_origins": allowed_origins,
            "private_origins": [] if private_origins is None else private_origins,
            "max_response_bytes": max_response_bytes,
        },
        allow_nan=False,
    )


class ExaSearch(SearchBackend):
    """Exa hosted MCP search, anonymous by default or with an explicit key.

    No automatic redirects, proxy environment, retries or fallback providers.
    The endpoint must use HTTPS, except explicit private origins for local tests.
    Decoded responses are bounded (512 KiB default, 16 MiB maximum). Network
    checks are shared with WebFetch. At most two decoders run per backend.
    """

    def __init__(
        self,
        *,
        api_key: str | None = None,
        endpoint: str | None = None,
        allowed_origins: list[str] | None = None,
        private_origins: list[str] | None = None,
        max_response_bytes: int = 512 * 1024,
    ) -> None:
        self._native = _native._SearchBackend.exa(
            api_key, _config(endpoint, allowed_origins, private_origins, max_response_bytes)
        )


class OllamaSearch(SearchBackend):
    """Ollama hosted REST search, with an explicit account API key.

    Same transport bounds and origin rules as ExaSearch. Maximum 10 results.
    Construction validates configuration without network access.
    """

    def __init__(
        self,
        *,
        api_key: str,
        endpoint: str | None = None,
        allowed_origins: list[str] | None = None,
        private_origins: list[str] | None = None,
        max_response_bytes: int = 512 * 1024,
    ) -> None:
        self._native = _native._SearchBackend.ollama(
            api_key, _config(endpoint, allowed_origins, private_origins, max_response_bytes)
        )


class OpenAISearch(SearchBackend):
    """Responses web search using an explicit model, key and request budgets.

    One request with required web search, store=False and no conversation history.
    Returns a cited answer only when a completed search action was observed.
    No observed search produces not_performed; incomplete answers are marked
    partial. Malformed/unfinished tool calls fail instead of reporting success.
    Cancellation stops local waiting, not necessarily remote generation or billing.

    API character spans are normalized to UTF-8 byte offsets in data. The text
    rendition adds visible Markdown citations. Render those links when displaying
    answers. limit bounds retained sources; cited sources are never silently lost.
    Same host-only transport settings and decoded-body bounds as ExaSearch.
    """

    def __init__(
        self,
        *,
        api_key: str,
        model: str,
        max_output_tokens: int = 2048,
        max_tool_calls: int = 1,
        endpoint: str | None = None,
        allowed_origins: list[str] | None = None,
        private_origins: list[str] | None = None,
        max_response_bytes: int = 512 * 1024,
    ) -> None:
        options = {
            "model": model,
            "max_output_tokens": max_output_tokens,
            "max_tool_calls": max_tool_calls,
        }
        self._native = _native._SearchBackend.openai(
            api_key,
            json.dumps(options, allow_nan=False),
            _config(endpoint, allowed_origins, private_origins, max_response_bytes),
        )


class WebSearchTool:
    """Standalone async dictionary API with an explicitly selected backend.

    Results, cited generated answers and non-execution have distinct payloads.
    Exa/Ollama return direct results; OpenAI returns an observed-search answer
    or explicit non-execution. Check partial/omitted counts.
    Sources and notices are untrusted data. Answers are never clipped through
    citations. Full serialized output is bounded; an oversized answer fails.
    Cancellation drops network work; bounded decoding may finish later.
    """

    def __init__(
        self,
        *,
        backend: SearchBackend,
        max_query_bytes: int | None = None,
        max_results: int | None = None,
        max_content_bytes: int | None = None,
        max_output_bytes: int | None = None,
        default_timeout_ms: int | None = None,
        max_timeout_ms: int | None = None,
    ) -> None:
        limits = {
            key: value
            for key, value in {
                "max_query_bytes": max_query_bytes,
                "max_results": max_results,
                "max_content_bytes": max_content_bytes,
                "max_output_bytes": max_output_bytes,
                "default_timeout_ms": default_timeout_ms,
                "max_timeout_ms": max_timeout_ms,
            }.items()
            if value is not None
        }
        self._native = _native._WebSearchTool(backend._native, json.dumps(limits, allow_nan=False))

    @classmethod
    async def create(
        cls,
        *,
        backend: SearchBackend,
        max_query_bytes: int | None = None,
        max_results: int | None = None,
        max_content_bytes: int | None = None,
        max_output_bytes: int | None = None,
        default_timeout_ms: int | None = None,
        max_timeout_ms: int | None = None,
    ) -> Self:
        """Construct without I/O, as the constructor does."""
        return cls(
            backend=backend,
            max_query_bytes=max_query_bytes,
            max_results=max_results,
            max_content_bytes=max_content_bytes,
            max_output_bytes=max_output_bytes,
            default_timeout_ms=default_timeout_ms,
            max_timeout_ms=max_timeout_ms,
        )

    @staticmethod
    def definition() -> dict[str, Any]:
        """Rust-derived schema without credentials, provider or endpoint arguments."""
        return json.loads(_native._WebSearchTool.definition_json())

    def capabilities(self) -> dict[str, Any]:
        """The selected backend's result kinds and service limits, for host inspection."""
        return json.loads(self._native.capabilities_json())

    async def call(self, arguments: dict[str, Any]) -> dict[str, Any]:
        """Execute one search, preserving typed errors and cancellation."""
        token = _native._Cancellation()
        try:
            return json.loads(
                await self._native.call(json.dumps(arguments, allow_nan=False), token)
            )
        finally:
            token.cancel()

    async def search(
        self,
        query: str,
        *,
        limit: int = 5,
        timeout_ms: int | None = None,
    ) -> dict[str, Any]:
        """Search for documents; excerpts need not be complete page text."""
        return await self.call({"query": query, "limit": limit, "timeout_ms": timeout_ms})
