# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Added `max_line_len` to `sse::frame::SseFrameParser`, 64 KiB by default.
- Added `max_event_len` to `sse::frame::SseFrameParser`, 1 MiB by default.
- Added `sse::frame::SseFrameParserError`.
- Added the event stream parser to the `response` fuzz target.
- Added a split against whole comparison to the `response` fuzz target.
- Added the challenge parser to the `response` fuzz target.
- Added `rfc9110::status::HttpStatusCode::is_informational`.
- Added `rfc9110::request::HttpRequestError`.
- Added `rfc3986::path::http_url_build`.

  It appends path segments to a base URL, each percent-encoded on its own, and refuses an empty segment, a dot segment and a segment holding a tab or a line break.

- Added `max_trailers_len` to `rfc9112::chunk::Http11ChunksRead`, 64 KiB by default.
- Added `max_trailers_len` to `rfc9112::chunk_stream::Http11ChunksReadStream`, 64 KiB by default.
- Added `rfc9112::read_headers::HTTP11_MAX_INTERIM_RESPONSES`.

### Changed

- **BREAKING**: Changed `sse::frame::SseFrameParser` to fail with `LineTooLarge` and `EventTooLarge`.

  Its `Return` is now `Result<Infallible, SseFrameParserError>` instead of `Infallible`.

- **BREAKING**: Changed `client::HttpClientError` to fail with `SseFrameParser`.
- **BREAKING**: Changed `rfc9112::chunk::Http11ChunksReadError` to fail with `InvalidChunkTerminator` and `TrailersTooLarge`.
- **BREAKING**: Changed `rfc9112::chunk_stream::Http11ChunksReadStreamError` to fail with `InvalidChunkTerminator` and `TrailersTooLarge`.
- **BREAKING**: Changed `rfc9112::send::Http11SendError` to fail with `UnsupportedTransferCoding`, `TooManyInterimResponses` and `InvalidRequest`.

  A final transfer coding other than `chunked` fails the exchange, a lone `identity` is ignored. Codings before `chunked` stay applied to the body.

- **BREAKING**: Changed `rfc1945::send::Http10SendError` to fail with `TooManyInterimResponses` and `InvalidRequest`.
- **BREAKING**: Changed `rfc9112::read_headers::Http11HeadersReadError` to fail with `TooManyInterimResponses`.
- **BREAKING**: Changed `HttpRequest::to_http_10_vec` and `HttpRequest::to_http_11_vec` to return a `Result`.

  They fail with `HttpRequestError` on a method or a header name that is not a token, a header value holding CR, LF or NUL, and a `Transfer-Encoding` header.

- Changed `rfc9112::read_headers::Http11HeadersRead` to skip up to 8 interim 1xx responses, 101 excepted.
- Changed HTTP/1.0 responses to be kept alive only on the `keep-alive` connection option.
- Changed the debug output of `rfc9110::response::HttpResponse` to print the body as its length.
- Changed the debug output of `rfc9110::request::HttpRequest` to hide the userinfo and the query values of its URL.

### Removed

- **BREAKING**: Removed the `Display` implementation of `rfc6750::bearer::HttpAuthBearer`.

  It wrote the raw token. Use `to_authorization` or `expose_secret`.

### Fixed

- Fixed sensitive response header values being written to the trace log.

  The head reader now logs the response through its redacting debug output.

- Fixed a quoted challenge parameter holding a space starting a new challenge.

  `rfc9110::challenge::HttpChallenge::parse_all` also accepts blanks around the equal sign.

- Fixed a fixed-length or single-chunk body peaking at three times its size in memory.
- Fixed the response head being parsed again at each read.

- Fixed event stream lines and events being buffered without bound.

  A server exceeding a limit now fails the parser, and ends `client::SseStream` with the error.

- Fixed a method, a header name or a header value adding lines to the request.

  `Http10Send` and `Http11Send` fail their first resume with `InvalidRequest`, before any write.

- Fixed chunk data being accepted without its CRLF.
- Fixed trailer fields being left unread after a chunked body.

  They are read and dropped, so they no longer start the next response of a reused connection.

- Fixed interim 1xx responses being returned as the final response.
- Fixed replies to HEAD requests waiting for a body.
- Fixed `chunked` being missed as the last of several transfer codings.
- Fixed `close` being missed among several connection options.
- Fixed `keep_alive` being true after a body read to EOF.
- Fixed conflicting content length values being accepted.

  They fail with `InvalidContentLength`. Repeated identical values are accepted.

- Fixed response heads over 64 headers failing to parse.

  The head size limit is now the only limit on the number of headers.

## [0.6.0] - 2026-11-03

### Added

- Added `max_headers_len` and `max_body_len` to `rfc1945::send::Http10Send` and `rfc9112::send::Http11Send`.

  They default to `rfc9112::read_headers::HTTP_DEFAULT_MAX_HEADERS_LEN` (64 KiB) and `rfc9110::send::HTTP_DEFAULT_MAX_BODY_LEN` (256 MiB).

- Added `max_len` to `rfc9112::read_headers::Http11HeadersRead`, 64 KiB by default.
- Added `max_body_len` to `rfc9112::chunk::Http11ChunksRead`, 256 MiB by default.
- Added `max_chunk_len` to `rfc9112::chunk_stream::Http11ChunksReadStream`, 256 MiB by default.
- Added a `response` fuzz target, run from the `fuzz` devShell.

### Changed

- **BREAKING**: Changed `rfc1945::send::Http10SendError` and `rfc9112::send::Http11SendError` to fail with `HeadersTooLarge` and `BodyTooLarge`.
- **BREAKING**: Changed `rfc9112::read_headers::Http11HeadersReadError` to fail with `TooLarge`.
- **BREAKING**: Changed `rfc9112::chunk::Http11ChunksReadError` to fail with `Eof`, `ChunkLineTooLarge` and `BodyTooLarge`.
- **BREAKING**: Changed `rfc9112::chunk_stream::Http11ChunksReadStreamError` to fail with `Eof`, `ChunkLineTooLarge` and `ChunkTooLarge`.
- **BREAKING**: Changed the base64 dependency to [base64-codec](https://crates.io/crates/base64-codec).

  `rfc7617::basic::HttpAuthBasicError::InvalidBase64` carries no error any more. `HttpAuthBasic::from_authorization` now accepts missing padding, non-zero trailing bits and blanks, and still refuses any other byte outside the alphabet.

- Changed the minimum Rust version to 1.89.

- Changed `sse::frame::SseFrameParser` to read buffered lines in place.

  Each line is found in one pass and consumed bytes are dropped once a resume, so a resume carrying many events or a long line runs up to 6 times faster.

- Changed `rfc9112::chunk::Http11ChunksRead` and `rfc9112::chunk_stream::Http11ChunksReadStream` to read buffered chunks in place.

  Consumed bytes are dropped once a resume instead of once a chunk, up to 8 times faster on small chunks.

### Removed

- Removed the `memchr` dependency.

### Fixed

- Fixed a chunk size near `usize::MAX` panicking the chunk readers.

  A hostile server could crash the client with `FFFFFFFFFFFFFFFF`. It now fails the exchange.

- Fixed response heads, bodies and chunk size lines being buffered without bound.

  A server exceeding a limit now fails the exchange with one of the `*TooLarge` errors.

- Fixed the send and chunk coroutines waiting forever on a connection closed mid-body.

  They now fail with `Eof`.

- Fixed `Content-Length` and chunk sizes accepting a leading `+`.
- Fixed `sse::frame::SseFrameParser` stripping a byte order mark after the first line.

## [0.5.0] - 2026-08-15

### Changed

- Bumped pimalaya-stream to 0.3, whose `Read` and `Write` retry a stream reporting it is not ready. **Behaviour change.**

  A blocking socket is not supposed to report `EAGAIN`, yet callers saw one surface mid-exchange and end the exchange with a bare `Resource temporarily unavailable (os error 35)`, macOS especially and the more readily the longer the exchange ran. The transport now retries such a failure for a minute before giving up with a `TimedOut` naming the budget, and arms a socket read deadline at connect time so a server going silent on a healthy connection stops blocking the caller forever.

  Its `StreamStd` is renamed `stream::Stream` and its connects take a per-transport options struct, which is what `HttpClientStd::connect` now calls. The `Tls` type that method takes comes from that version too, so a consumer must move with it.

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

[unreleased]: https://github.com/pimalaya/io-http/compare/v0.6.0..HEAD
[0.6.0]: https://github.com/pimalaya/io-http/compare/v0.5.0..v0.6.0
[0.5.0]: https://github.com/pimalaya/io-http/compare/v0.4.0..v0.5.0
[0.4.0]: https://github.com/pimalaya/io-http/compare/v0.3.0..v0.4.0
[0.3.0]: https://github.com/pimalaya/io-http/compare/v0.2.0..v0.3.0
[0.2.0]: https://github.com/pimalaya/io-http/compare/v0.1.1..v0.2.0
[0.1.1]: https://github.com/pimalaya/io-http/compare/v0.1.0..v0.1.1
[0.1.0]: https://github.com/pimalaya/io-http/compare/v0.0.3..v0.1.0
[0.0.3]: https://github.com/pimalaya/io-http/compare/v0.0.2..v0.0.3
[0.0.2]: https://github.com/pimalaya/io-http/compare/v0.0.1..v0.0.2
[0.0.1]: https://github.com/pimalaya/io-http/compare/root..v0.0.1
