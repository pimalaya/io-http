# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Added the `HttpCoroutine` trait mirroring `core::ops::Coroutine`.

  Composed of `Yield` and `Return` associated types and a two-variant `HttpCoroutineState<Y, R>` (`Yielded(Y)` and `Complete(R)`). Standard coroutines pick the shared `HttpYield { WantsRead, WantsWrite(Vec<u8>) }`; redirect-aware and streaming coroutines declare their own `Yield` (`HttpSendYield::WantsRedirect`, `Http11ReadChunksStreamYield::Frame`, `SseFrameParserYield::Frame`).

- Added the `http_try!` macro: coroutine equivalent of `?`.

  Advances one inner resume step, re-yields intermediate `Yielded(y)` (via `Into`), and short-circuits on `Complete(Err(_))`.

- Added I/O-free HTTP/1.0 request-response coroutine following RFC 1945.

  Same shape as `Http11Send` but without chunked transfer coding; connections close after each response unless the server returns `Connection: keep-alive`.

- Added I/O-free HTTP/1.1 request-response coroutine following RFC 9112.

  Serialises the request, drives the head parse via `Http11ReadHeaders`, then selects a body strategy from the response headers: chunked, content-length, or read-to-EOF. Surfaces `HttpSendYield::WantsRedirect` on 3xx responses with a parseable `Location`.

- Added I/O-free HTTP/1.X response-head parser following RFC 9112 §6.

  Extracted from `Http11Send` so downstream consumers can drive the response-head parse standalone. Both send coroutines and `HttpClientStd::send_streaming` delegate to it.

- Added I/O-free HTTP/1.1 chunked-transfer body decoder following RFC 9112 §7.1.

  `Http11ReadChunks` accumulates the whole body into a single `Vec<u8>`; `Http11ReadChunksStream` yields each decoded chunk as soon as its body bytes are available (suitable for SSE and other long-lived streaming responses).

- Added I/O-free Server-Sent Events frame parser following the W3C HTML Living Standard.

  `SseFrameParser` + `SseFrame` in `sse::frame`. Line-oriented and infallible (`Return = Infallible`); driver stops resuming when the underlying body stream closes.

- Added I/O-free `.well-known` URI discovery coroutine following RFC 8615.

  Wraps `Http11Send` and surfaces the resolved redirect URL as part of the terminal output.

- Added HTTP Basic credential helper following RFC 7617.

  `BasicCredentials::new(username, password)` + `to_authorization()` / `from_authorization()`. Password stored as `SecretString` (redacted in `Debug`, zeroed on drop).

- Added HTTP Bearer credential helper following RFC 6750.

  `BearerToken::new(token)` + `to_authorization()` / `from_authorization()`. Token stored as `SecretString`.

- Added the `client` cargo feature enabling `HttpClientStd::new(stream)`.

  Blocking light client wrapping any `Read + Write + Send` stream and exposing `send` (HTTP/1.1) / `send_http10` / `send_streaming` (SSE) / `run` (generic coroutine driver).

- Added `HttpClientStd::send_streaming(self, request) -> SseStream`.

  Consumes the client, sends the request, requires `Transfer-Encoding: chunked`, and returns a long-lived iterator of decoded `SseFrame` events. Pre-read body bytes are forwarded automatically.

- Added the `rustls-ring` cargo feature (default) enabling `HttpClientStd::connect(url, tls)`.

  Opens `http://` (plain TCP) or `https://` (implicit TLS) via [pimalaya/stream](https://github.com/pimalaya/stream) with rustls + ring crypto provider.

- Added the `rustls-aws` cargo feature.

  Same full client as `rustls-ring` but with the aws-lc-rs crypto provider.

- Added the `native-tls` cargo feature.

  Same full client backed by the platform's `native-tls` implementation.

- Added the `vendored` cargo feature.

  Compiles the underlying TLS dependencies in vendored mode (forwarded to `pimalaya-stream/vendored`).

### Changed

- Organised source into RFC-numbered folders (`rfc1945/`, `rfc6750/`, `rfc7617/`, `rfc8615/`, `rfc9110/`, `rfc9112/`).

- Normalised error messages to the uniform `"<protocol> <verb> failed: <detail>"` pattern across every coroutine error enum.

- Replaced every per-coroutine `*Result` enum with `HttpCoroutineState<Y, R>`; per-coroutine `Output` structs (`HttpSendOutput`, `Http11ReadChunksOutput`, `Http11ReadHeadersOutput`, `WellKnownOutput`) hold the previous `Ok { … }` fields. Shared `HttpSendOutput` and `HttpSendYield` moved to `rfc9110::send`, reused by both `Http10Send` and `Http11Send`.

## [0.0.3] - 2025-10-24

### Added

- Add missing deny.toml

### Changed

- Bump dependencies

### Fixed

- Handle 204 response status code

## [0.0.2] - 2025-08-04

### Changed

- Clean the whole lib
- Release v0.0.2

## [0.0.1] - 2025-06-04

### Added

- Init HTTP 1.1 module with send coroutine

[unreleased]: https://github.com/pimalaya/io-http/compare/v0.0.3..HEAD
[0.0.3]: https://github.com/pimalaya/io-http/compare/v0.0.2..v0.0.3
[0.0.2]: https://github.com/pimalaya/io-http/compare/v0.0.1..v0.0.2
[0.0.1]: https://github.com/pimalaya/io-http/compare/root..v0.0.1
