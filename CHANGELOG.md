# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Add support for HTTP/1.0.
- Add streaming chunked-transfer reader `Http11ReadChunksStream` in `rfc9112::chunk_stream` that yields each decoded chunk as soon as it arrives instead of buffering the whole body.
- Add W3C Server-Sent Events client: frame parser `SseFrameParser` + `SseFrame` in `sse::frame`, plus std-blocking driver `HttpClientStd::send_streaming` returning a long-lived `SseStream` iterator.
- Extracted `Http11ReadHeaders` coroutine in `rfc9112::read_headers` so downstream consumers can drive the response-head parse without going through `Http11Send`. `Http11Send`, `Http10Send`, and `HttpClientStd::send_streaming` all delegate to it; previously each duplicated the `httparse` + `Connection` header parse inline.
- Add unified `HttpCoroutine` trait + two-variant `CoroutineState<Y, R>` (`Yielded` / `Complete`) in `crate::coroutine`, mirroring std's `core::ops::Coroutine` shape. Most coroutines pick the standard `HttpYield { WantsRead, WantsWrite(Vec<u8>) }`; coroutines that need extra intermediate variants declare their own (`HttpSendYield::WantsRedirect`, `Http11ReadChunksStreamYield::Frame`, `SseFrameParserYield::Frame`). `HttpClientStd::run<C>` drives any standard-Yield coroutine generically.

### Changed

- Organize code into RFC folders.
- Replaced every per-coroutine `Smtp*Result` / `Http*SendResult` / `Http11ReadChunks*Result` / `WellKnownResult` / `SseFrameParserResult` / `Http11ReadHeadersResult` enum with the generator-shape `CoroutineState<Y, R>`. Each coroutine now implements `HttpCoroutine` directly; `resume` returns `CoroutineState::Yielded(<per-coroutine yield>)` or `CoroutineState::Complete(Result<Output, Error>)`. Per-coroutine `*Ok` payload structs (`Http11ReadChunksOutput`, `Http11ReadHeadersOutput`, `WellKnownOutput`) hold the previous `Ok { … }` fields. Shared `HttpSendOutput` and `HttpSendYield` moved to `rfc9110::send`, reused by both `Http10Send` and `Http11Send`.

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

[unreleased]: https://github.com/pimalaya/io-dns/compare/v0.0.3..HEAD
[0.0.3]: https://github.com/pimalaya/io-http/compare/v0.0.2..v0.0.3
[0.0.2]: https://github.com/pimalaya/io-http/compare/v0.0.1..v0.0.2
[0.0.1]: https://github.com/pimalaya/io-http/compare/root..v0.0.1
