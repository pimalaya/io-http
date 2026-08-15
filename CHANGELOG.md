# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- Bumped pimalaya-stream to 0.3, whose `Read` and `Write` retry a stream reporting it is not ready. **Behaviour change.**

  A blocking socket is not supposed to report `EAGAIN`, yet callers saw one surface mid-exchange and end the exchange with a bare `Resource temporarily unavailable (os error 35)`, macOS especially and the more readily the longer the exchange ran. The transport now retries such a failure for a minute before giving up with a `TimedOut` naming the budget, and arms a socket read deadline at connect time so a server going silent on a healthy connection stops blocking the caller forever. Its `StreamStd` is renamed `stream::Stream` and its connects take a per-transport options struct, which is what this crate now calls.

## [0.4.0] - 2026-08-15

### Added

- Added the `client::HttpClient` and `client::HttpClientAsync` traits, turning the coroutines every client drives identically into defaulted methods.

  Two pumps are required rather than one: `run` for the plain read/write coroutines, `run_send` for the request ones, whose `HttpSendYield::WantsRedirect` is a policy the implementor owns. The async trait returns `impl Future<..> + Send` so its defaults survive `tokio::spawn`; the blocking one carries no `Send` bound, leaving room for a thread-affine client such as a JNI bridge.

- Added the tokio client example, implementing `HttpClientAsync` over a tokio socket and tokio-rustls.

### Changed

- Moved `run`, `send` and `send_http10` off `HttpClientStd` onto the `HttpClient` trait. **Breaking.**

  Callers add `use io_http::client::HttpClient;`. `send_streaming` stays inherent, since consuming the client for an `SseStream` iterator is a runtime-specific choice.

- Renamed `client::HttpClientStdError` to `client::HttpClientError` and gave it a `Transport` variant. **Breaking.**

  It is the error type of both traits now, and `Transport` boxes an error for implementors whose I/O is not `std::io::Error`.

- Bumped pimalaya-stream to 0.2, which drops its SASL module. The `Tls` type `HttpClientStd::connect` takes comes from that version.
- Bumped base64 to 0.23.
- Raised the minimum supported Rust version from 1.87 to 1.88.

## [0.3.0] - 2026-07-15

### Changed

- Renamed the verb-first coroutines to the target-first naming convention: `Http11ReadHeaders` is now `Http11HeadersRead`, `Http11ReadChunks` is now `Http11ChunksRead` and `Http11ReadChunksStream` is now `Http11ChunksReadStream`, their `Error`/`Output`/`Yield` companions along.
- Prefixed the header name constants with `HTTP_` (`HTTP_HOST`, `HTTP_AUTHORIZATION`, `HTTP_SENSITIVE_HEADERS`, ...); moved challenge parsing onto `HttpChallenge::parse_all` (was the free `parse_challenges`) and the default ALPN onto `HttpClientStd::default_alpn` (was the free `default_alpn`); the character constants module is no longer public.
- Aligned logging with the Pimalaya guidelines: send coroutines log their request preparation as a debug plus a data trace, body-reading state changes were promoted from trace to debug, and the SSE parser no longer logs at the top of every resume.
- Bumped pimalaya-stream to 0.1.

## [0.2.0] - 2026-07-13

### Added

- Added the rfc9110 challenge module: `parse_challenges` turns a `WWW-Authenticate` header value into its `HttpChallenge` list (lowercased scheme, auth parameters unquoted with quoted-comma and escape handling, `token68` blobs skipped), `HttpChallenge::param` looks parameters up case-insensitively, and `HttpResponse::challenges` gathers every `WWW-Authenticate` header of a response. This is the shared primitive behind io-pim-discovery's authentication probe (scheme discovery, PACC §5.4.2) and its RFC 9728 `resource_metadata` extraction, which previously each parsed the header themselves.

### Fixed

- Generated a `Host` header from the request URL in the HTTP/1.1 serialiser when the caller did not set one.

  RFC 9112 §3.2 requires `Host` on every HTTP/1.1 request, and servers commonly answer 400 without it (nginx does), which silently broke the RFC 8615 `.well-known` probe: posteo.net's carddav redirect was never seen and discovery fell back to the bare origin. The generated value keeps a non-default port and callers that set `Host` themselves (io-webdav) are untouched.

### Changed

- Renamed `BasicCredentials` to `HttpAuthBasic` and `BearerToken` to `HttpAuthBearer`.

  The credential types now carry the crate's `Http*` prefix so they read clearly when wrapped by higher-level crates (e.g. io-webdav's `WebdavAuth::Basic(HttpAuthBasic)`). The module paths (`rfc7617::basic`, `rfc6750::bearer`) and the `new` / `to_authorization` / `from_authorization` API are unchanged.

- Renamed the remaining public types that were missing the crate prefix: `StatusCode` to `HttpStatusCode`, `BasicError` to `HttpAuthBasicError`, `BearerError` to `HttpAuthBearerError`, and the RFC 8615 `WellKnown` / `WellKnownOutput` / `WellKnownError` coroutine types to `Http11WellKnown` / `Http11WellKnownOutput` / `Http11WellKnownError`.

  Every public type now carries the `Http` prefix (or `Http11` for HTTP/1.1-specific machinery): the auth error enums line up with their companion `HttpAuthBasic` / `HttpAuthBearer` structs, and `Http11WellKnown` reflects that it wraps `Http11Send`. The `Sse*` parser family keeps its own deliberate namespace. RFC module paths and the public API are otherwise unchanged.

- Replaced the per-iteration `State` trace dump with a descriptive `trace!` line emitted on each state transition, and dropped the `fmt::Display` impls on the private `State` enums.

  Send and chunked-body coroutines now log a readable line when they advance (e.g. "looking for body of length 42", "reading chunked body", "reading chunk of 6 bytes") instead of printing the state name on every `resume`. Messages no longer carry a protocol prefix, since the `log` target already records it.

## [0.1.1] - 2026-06-03

### Fixed

- Removed the `doc_auto_cfg` nightly feature gate from `lib.rs`. It was removed from rustc in 1.92.0 (merged into `doc_cfg`) and broke the docs.rs build for v0.1.0.

## [0.1.0] - 2026-06-03

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

- Each per-coroutine private `State` enum now implements `fmt::Display`; resume bodies emit a single `trace!("<proto> <verb>: {state}")` per iteration in place of the previous scattered per-branch traces.

- Tightened inline documentation across all modules: concise module headers with markdown RFC footnote links, one-line `///` per public type, struct-level coroutine examples moved up to module headers.

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

[unreleased]: https://github.com/pimalaya/io-http/compare/v0.4.0..HEAD
[0.4.0]: https://github.com/pimalaya/io-http/compare/v0.3.0..v0.4.0
[0.3.0]: https://github.com/pimalaya/io-http/compare/v0.2.0..v0.3.0
[0.2.0]: https://github.com/pimalaya/io-http/compare/v0.1.1..v0.2.0
[0.1.1]: https://github.com/pimalaya/io-http/compare/v0.1.0..v0.1.1
[0.1.0]: https://github.com/pimalaya/io-http/compare/v0.0.3..v0.1.0
[0.0.3]: https://github.com/pimalaya/io-http/compare/v0.0.2..v0.0.3
[0.0.2]: https://github.com/pimalaya/io-http/compare/v0.0.1..v0.0.2
[0.0.1]: https://github.com/pimalaya/io-http/compare/root..v0.0.1
