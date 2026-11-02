# roomkit-tools-http

Internal to roomkit-tools: the HTTP transport `roomkit-tools-web` and
`roomkit-tools-search` share, with DNS-pinned connections, an origin policy
that refuses private networks by default, no automatic redirects or proxies,
and bounded response bodies. Its items are hidden and are not a public API:
they may change in any release.

Configure origins through the tools instead, with `WebFetchPolicy` in
`roomkit-tools-web` and `SearchHttpConfig` in `roomkit-tools-search`.

## License

MIT. Part of [roomkit-tools](https://github.com/roomkit-live/roomkit-tools),
agent tools a host selects one by one, also available in Python. The
[Rust guide](https://github.com/roomkit-live/roomkit-tools/blob/main/guide/rust.md)
describes every crate.
