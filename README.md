# I/O HTTP [![Documentation](https://img.shields.io/docsrs/io-http?style=flat&logo=docs.rs&logoColor=white)](https://docs.rs/io-http/latest/io_http) [![Matrix](https://img.shields.io/badge/chat-%23pimalaya-blue?style=flat&logo=matrix&logoColor=white)](https://matrix.to/#/#pimalaya:matrix.org) [![Mastodon](https://img.shields.io/badge/news-%40pimalaya-blue?style=flat&logo=mastodon&logoColor=white)](https://fosstodon.org/@pimalaya)

HTTP/1.X client library, written in Rust

This library is composed of 3 feature-gated layers:

- Low-level **I/O-free** coroutines: these `no_std`-compatible state machines contain the whole HTTP/1.X logic and can be used anywhere
- Mid-level **light client**: a standard, blocking HTTP client using a `Stream: Read + Write`
- High-level **full client**: light client + TCP connections and TLS negotiations handled for you

## Table of contents

- [Features](#features)
- [RFC coverage](#rfc-coverage)
- [Usage](#usage)
  - [Coroutine](#coroutine)
  - [Light client](#light-client)
  - [Full client](#full-client)
- [Examples](#examples)
- [AI disclosure](#ai-disclosure)
- [License](#license)
- [Social](#social)
- [Sponsoring](#sponsoring)

## Features

- **I/O-free** coroutines: `no_std` state machines; no sockets, no async runtime, no `std` required, drive against any blocking, async, or fuzz harness.
- Light standard, blocking client (requires `client` feature)
- Full standard, blocking client with **TLS** support:
  - [Rustls](https://crates.io/crates/rustls) with ring crypto (requires `rustls-ring` feature)
  - [Rustls](https://crates.io/crates/rustls) with aws crypto (requires `rustls-aws` feature)
  - [Native TLS](https://crates.io/crates/native-tls) (requires `native-tls` feature)
- **HTTP versions**: HTTP/1.0 (RFC 1945) and HTTP/1.1 (RFC 9112) with fixed-length, chunked, and read-to-EOF body strategies.
- **Streaming**: chunked decoder (`Http11ReadChunksStream`) + W3C Server-Sent Events parser (`SseFrameParser`) + std-blocking driver (`HttpClientStd::send_streaming`).
- **Authentication helpers**: `Authorization: Bearer <token>` (RFC 6750) and `Authorization: Basic <base64(user:pass)>` (RFC 7617).
- **`.well-known` discovery** (RFC 8615) shipped as a dedicated coroutine.

> [!TIP]
> I/O HTTP is written in [Rust](https://www.rust-lang.org/) and uses [cargo features](https://doc.rust-lang.org/cargo/reference/features.html) to gate backend support. The default feature set is declared in [Cargo.toml](./Cargo.toml) or on [docs.rs](https://docs.rs/crate/io-http/latest/features).

## RFC coverage

| Module   | What it covers                                                                                                |
|----------|---------------------------------------------------------------------------------------------------------------|
| [1945]   | HTTP/1.0: request/response coroutine (`Http10Send`)                                                           |
| [6750]   | OAuth 2.0 Bearer token: `Authorization: Bearer <token>`                                                       |
| [7617]   | HTTP Basic authentication: `Authorization: Basic <base64(user:pass)>`                                         |
| [8615]   | `.well-known` URI discovery: `WellKnown` coroutine                                                            |
| [9110]   | HTTP semantics: shared types `HttpRequest`, `HttpResponse`, `StatusCode`, `HttpSendOutput`, `HttpSendYield`   |
| [9112]   | HTTP/1.1: request/response (`Http11Send`), response-head (`Http11ReadHeaders`), chunked transfer (whole-body + streaming) |
| `sse`    | W3C Server-Sent Events frame parser (HTML Living Standard)                                                    |

[1945]: https://www.rfc-editor.org/rfc/rfc1945
[6750]: https://www.rfc-editor.org/rfc/rfc6750
[7617]: https://www.rfc-editor.org/rfc/rfc7617
[8615]: https://www.rfc-editor.org/rfc/rfc8615
[9110]: https://www.rfc-editor.org/rfc/rfc9110
[9112]: https://www.rfc-editor.org/rfc/rfc9112

## Usage

I/O-HTTP can be consumed three ways, depending on how much of the I/O stack you want to own. Each mode is gated by cargo features.

Whichever mode you pick, every coroutine implements the `HttpCoroutine` trait. Its `resume(arg: Option<&[u8]>)` method returns a `HttpCoroutineState<Yield, Return>` with two variants:

- `Yielded(Y)`: intermediate. `Y` is the per-coroutine yield type (e.g. `HttpYield::WantsRead` / `HttpYield::WantsWrite(Vec<u8>)` for most coroutines; `HttpSendYield::WantsRedirect { … }` for `Http*Send`; `SseFrameParserYield::Frame(SseFrame)` for the SSE parser).
- `Complete(R)`: terminal. By convention `R = Result<Output, Error>` carrying the operation's final value.

### Coroutine

No features required: works in `#![no_std]`, no sockets, no async runtime. You own the loop and the bytes; the library only produces request bytes and consumes server responses.

Send an HTTP/1.1 request against an async Tokio + rustls stack (the same shape works under blocking, fuzzing, or in-memory replay):

```rust,no_run
use std::sync::Arc;

use io_http::{
    coroutine::*,
    rfc9110::{request::HttpRequest, send::HttpSendYield},
    rfc9112::send::Http11Send,
};
use rustls::ClientConfig;
use rustls_platform_verifier::ConfigVerifierExt;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use tokio_rustls::TlsConnector;
use url::Url;

#[tokio::main]
async fn main() {
    let url = Url::parse("https://example.com/").unwrap();
    let domain = url.domain().unwrap().to_owned();
    let port = url.port_or_known_default().unwrap_or(443);

    let config = Arc::new(ClientConfig::with_platform_verifier().unwrap());
    let connector = TlsConnector::from(config);
    let server_name = domain.clone().try_into().unwrap();
    let tcp = TcpStream::connect((domain.as_str(), port)).await.unwrap();
    let mut stream = connector.connect(server_name, tcp).await.unwrap();

    let request = HttpRequest::get(url)
        .header("Host", &domain)
        .header("Connection", "close");

    let mut send = Http11Send::new(request);
    let mut arg: Option<&[u8]> = None;
    let mut buf = [0u8; 4096];

    let response = loop {
        match send.resume(arg.take()) {
            HttpCoroutineState::Complete(Ok(out)) => break out.response,
            HttpCoroutineState::Complete(Err(err)) => panic!("{err}"),
            HttpCoroutineState::Yielded(HttpSendYield::WantsRead) => {
                let n = stream.read(&mut buf).await.unwrap();
                arg = Some(&buf[..n]);
            }
            HttpCoroutineState::Yielded(HttpSendYield::WantsWrite(bytes)) => {
                stream.write_all(&bytes).await.unwrap();
            }
            HttpCoroutineState::Yielded(HttpSendYield::WantsRedirect { url, .. }) => {
                panic!("redirect to {url}");
            }
        }
    };

    println!("{} {}", response.version, *response.status);
}
```

### Light client

Enable the `client` feature. `HttpClientStd::new(stream)` wraps any blocking `Read + Write` and exposes `send` / `send_http10` / `send_streaming`. You still open the TCP socket and run TLS yourself, then hand over a ready-to-talk stream; the client takes it from there.

```toml,ignore
[dependencies]
io-http = { version = "0.1.1", default-features = false, features = ["client"] }
```

```rust,no_run
# fn main() -> Result<(), Box<dyn std::error::Error>> {
use std::{net::TcpStream, sync::Arc};

use io_http::{client::HttpClientStd, rfc9110::request::HttpRequest};
use rustls::{ClientConfig, ClientConnection, StreamOwned};
use rustls_platform_verifier::ConfigVerifierExt;
use url::Url;

let url = Url::parse("https://example.com/")?;
let domain = url.domain().unwrap();

let config = ClientConfig::with_platform_verifier()?;
let server_name = domain.to_string().try_into()?;
let conn = ClientConnection::new(Arc::new(config), server_name)?;
let tcp = TcpStream::connect((domain, 443))?;
let stream = StreamOwned::new(conn, tcp);

let mut client = HttpClientStd::new(stream);

let request = HttpRequest::get(url.clone())
    .header("Host", domain)
    .header("Connection", "close");

let output = client.send(request)?;
println!("{} {}", output.response.version, *output.response.status);
# Ok(())
# }
```

### Full client

Enable one of the TLS feature flags: `rustls-ring` (default), `rustls-aws`, or `native-tls`. `HttpClientStd::connect(url, tls)` opens `http://` (plain TCP) or `https://` (implicit TLS) via [pimalaya/stream](https://github.com/pimalaya/stream), returning a ready-to-use client.

```toml,ignore
[dependencies]
io-http = "0.1.1" # rustls-ring is enabled by default
```

```rust,no_run
# fn main() -> Result<(), Box<dyn std::error::Error>> {
use io_http::{client::HttpClientStd, rfc9110::request::HttpRequest};
use pimalaya_stream::tls::Tls;
use url::Url;

let url = Url::parse("https://example.com/")?;
let tls = Tls::default();
let mut client = HttpClientStd::connect(&url, &tls)?;

let request = HttpRequest::get(url.clone())
    .header("Host", url.host_str().unwrap())
    .header("Connection", "close");

let output = client.send(request)?;
println!("{} {}", output.response.version, *output.response.status);
# Ok(())
# }
```

## Examples

See complete examples at [./examples](https://github.com/pimalaya/io-http/blob/master/examples).

Have also a look at real-world projects built on top of this library:

- [io-jmap](https://github.com/pimalaya/io-jmap): Set of I/O-free Rust coroutines to manage JMAP sessions
- [io-addressbook](https://github.com/pimalaya/io-addressbook): Set of I/O-free coroutines to manage contacts
- [io-oauth](https://github.com/pimalaya/io-oauth): Set of I/O-free Rust coroutines to manage OAuth flows
- [io-starttls](https://github.com/pimalaya/io-starttls): I/O-free Rust coroutine to upgrade any plain stream to a secure one
- [Cardamum](https://github.com/pimalaya/cardamum): CLI to manage contacts
- [Ortie](https://github.com/pimalaya/ortie): CLI to manage OAuth access tokens

## AI disclosure

This project is developed with AI assistance. This section documents how, so users and downstream packagers can make informed decisions.

- **Tools**: Claude Code (Anthropic), Opus 4.7, invoked locally with a persistent project-scoped memory and a small set of repo-specific rules.

- **Used for**: Refactors, mechanical multi-file edits, boilerplate (feature gates, error enums, derive macros, trait impls), test scaffolding, doc polish, exploratory design conversations.

- **Not used for**: Engineering, critical code, git manipulation (commit, merge, rebase…), real-world tests.

- **Verification**: Every AI-assisted change is read, compiled, tested, and formatted before commit (`nix develop --command cargo check / cargo test / cargo fmt`). Behavioural correctness is verified against the relevant RFC or upstream spec, not assumed from the model output. Tests are never adjusted to fit AI-generated code; the code is adjusted to fit correct behaviour.

- **Limitations**: AI models occasionally produce code that compiles and passes tests but is subtly wrong: off-by-one errors, missed edge cases, plausible but nonexistent APIs, stale RFC references. The verification workflow catches most of this; it does not catch all of it. Bug reports are welcome and taken seriously.

- **Last reviewed**: 30/05/2026

## License

This project is licensed under either of:

- [MIT license](LICENSE-MIT)
- [Apache License, Version 2.0](LICENSE-APACHE)

at your option.

## Social

- Chat on [Matrix](https://matrix.to/#/#pimalaya:matrix.org)
- News on [Mastodon](https://fosstodon.org/@pimalaya) or [RSS](https://fosstodon.org/@pimalaya.rss)
- Mail at [pimalaya.org@posteo.net](mailto:pimalaya.org@posteo.net)

## Sponsoring

[![nlnet](https://nlnet.nl/logo/banner-160x60.png)](https://nlnet.nl/)

Special thanks to the [NLnet foundation](https://nlnet.nl/) and the [European Commission](https://www.ngi.eu/) that have been financially supporting the project for years:

- 2022 → 2023: [NGI Assure](https://nlnet.nl/project/Himalaya/)
- 2023 → 2024: [NGI Zero Entrust](https://nlnet.nl/project/Pimalaya/)
- 2024 → 2026: [NGI Zero Core](https://nlnet.nl/project/Pimalaya-PIM/)
- *2027 in preparation…*

If you appreciate the project, feel free to donate using one of the following providers:

[![GitHub](https://img.shields.io/badge/-GitHub%20Sponsors-fafbfc?logo=GitHub%20Sponsors)](https://github.com/sponsors/soywod)
[![Ko-fi](https://img.shields.io/badge/-Ko--fi-ff5e5a?logo=Ko-fi&logoColor=ffffff)](https://ko-fi.com/soywod)
[![Buy Me a Coffee](https://img.shields.io/badge/-Buy%20Me%20a%20Coffee-ffdd00?logo=Buy%20Me%20A%20Coffee&logoColor=000000)](https://www.buymeacoffee.com/soywod)
[![Liberapay](https://img.shields.io/badge/-Liberapay-f6c915?logo=Liberapay&logoColor=222222)](https://liberapay.com/soywod)
[![thanks.dev](https://img.shields.io/badge/-thanks.dev-000000?logo=data:image/svg+xml;base64,PHN2ZyB3aWR0aD0iMjQuMDk3IiBoZWlnaHQ9IjE3LjU5NyIgY2xhc3M9InctMzYgbWwtMiBsZzpteC0wIHByaW50Om14LTAgcHJpbnQ6aW52ZXJ0IiB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciPjxwYXRoIGQ9Ik05Ljc4MyAxNy41OTdINy4zOThjLTEuMTY4IDAtMi4wOTItLjI5Ny0yLjc3My0uODktLjY4LS41OTMtMS4wMi0xLjQ2Mi0xLjAyLTIuNjA2di0xLjM0NmMwLTEuMDE4LS4yMjctMS43NS0uNjc4LTIuMTk1LS40NTItLjQ0Ni0xLjIzMi0uNjY5LTIuMzQtLjY2OUgwVjcuNzA1aC41ODdjMS4xMDggMCAxLjg4OC0uMjIyIDIuMzQtLjY2OC40NTEtLjQ0Ni42NzctMS4xNzcuNjc3LTIuMTk1VjMuNDk2YzAtMS4xNDQuMzQtMi4wMTMgMS4wMjEtMi42MDZDNS4zMDUuMjk3IDYuMjMgMCA3LjM5OCAwaDIuMzg1djEuOTg3aC0uOTg1Yy0uMzYxIDAtLjY4OC4wMjctLjk4LjA4MmExLjcxOSAxLjcxOSAwIDAgMC0uNzM2LjMwN2MtLjIwNS4xNTYtLjM1OC4zODQtLjQ2LjY4Mi0uMTAzLjI5OC0uMTU0LjY4Mi0uMTU0IDEuMTUxVjUuMjNjMCAuODY3LS4yNDkgMS41ODYtLjc0NSAyLjE1NS0uNDk3LjU2OS0xLjE1OCAxLjAwNC0xLjk4MyAxLjMwNXYuMjE3Yy44MjUuMyAxLjQ4Ni43MzYgMS45ODMgMS4zMDUuNDk2LjU3Ljc0NSAxLjI4Ny43NDUgMi4xNTR2MS4wMjFjMCAuNDcuMDUxLjg1NC4xNTMgMS4xNTIuMTAzLjI5OC4yNTYuNTI1LjQ2MS42ODIuMTkzLjE1Ny40MzcuMjYuNzMyLjMxMi4yOTUuMDUuNjIzLjA3Ni45ODQuMDc2aC45ODVabTE0LjMxNC03LjcwNmgtLjU4OGMtMS4xMDggMC0xLjg4OC4yMjMtMi4zNC42NjktLjQ1LjQ0Ni0uNjc3IDEuMTc3LS42NzcgMi4xOTVWMTQuMWMwIDEuMTQ0LS4zNCAyLjAxMy0xLjAyIDIuNjA2LS42OC41OTMtMS42MDUuODktMi43NzQuODloLTIuMzg0di0xLjk4OGguOTg0Yy4zNjIgMCAuNjg4LS4wMjcuOTgtLjA4LjI5Mi0uMDU1LjUzOC0uMTU3LjczNy0uMzA4LjIwNC0uMTU3LjM1OC0uMzg0LjQ2LS42ODIuMTAzLS4yOTguMTU0LS42ODIuMTU0LTEuMTUydi0xLjAyYzAtLjg2OC4yNDgtMS41ODYuNzQ1LTIuMTU1LjQ5Ny0uNTcgMS4xNTgtMS4wMDQgMS45ODMtMS4zMDV2LS4yMTdjLS44MjUtLjMwMS0xLjQ4Ni0uNzM2LTEuOTgzLTEuMzA1LS40OTctLjU3LS43NDUtMS4yODgtLjc0NS0yLjE1NXYtMS4wMmMwLS40Ny0uMDUxLS44NTQtLjE1NC0xLjE1Mi0uMTAyLS4yOTgtLjI1Ni0uNTI2LS40Ni0uNjgyYTEuNzE5IDEuNzE5IDAgMCAwLS43MzctLjMwNyA1LjM5NSA1LjM5NSAwIDAgMC0uOTgtLjA4MmgtLjk4NFYwaDIuMzg0YzEuMTY5IDAgMi4wOTMuMjk3IDIuNzc0Ljg5LjY4LjU5MyAxLjAyIDEuNDYyIDEuMDIgMi42MDZ2MS4zNDZjMCAxLjAxOC4yMjYgMS43NS42NzggMi4xOTUuNDUxLjQ0NiAxLjIzMS42NjggMi4zNC42NjhoLjU4N3oiIGZpbGw9IiNmZmYiLz48L3N2Zz4=)](https://thanks.dev/soywod)
[![PayPal](https://img.shields.io/badge/-PayPal-0079c1?logo=PayPal&logoColor=ffffff)](https://www.paypal.com/paypalme/soywod)
