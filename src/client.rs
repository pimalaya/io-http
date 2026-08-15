//! # HTTP client surfaces
//!
//! Two traits and one implementation of them. [`HttpClient`] and
//! [`HttpClientAsync`] carry the request surface: implement the two
//! pumps and inherit the commands. [`HttpClientStd`] is the opinionated
//! blocking implementation, wrapping a boxed `Read + Write + Send`
//! stream.
//!
//! Each [`send`] / [`send_http10`] is self-contained, HTTP having no
//! session context. With a TLS feature enabled,
//! [`HttpClientStd::connect`] opens `http://` / `https://` URLs
//! end-to-end via [`pimalaya_stream::stream::Stream`].
//!
//! [`send`]: HttpClient::send
//! [`send_http10`]: HttpClient::send_http10

use core::{future::Future, mem};

use alloc::{boxed::Box, string::String, vec, vec::Vec};

use std::io::{self, Read, Write};

use thiserror::Error;
use url::Url;

use crate::{
    coroutine::*,
    rfc1945::send::*,
    rfc9110::{
        headers::HTTP_TRANSFER_ENCODING,
        request::HttpRequest,
        response::HttpResponse,
        send::{HttpSendOutput, HttpSendYield},
    },
    rfc9112::{chunk_stream::*, read_headers::*, send::*},
    sse::frame::*,
};

#[cfg(any(
    feature = "rustls-aws",
    feature = "rustls-ring",
    feature = "native-tls"
))]
mod connect;

const READ_BUFFER_SIZE: usize = 16 * 1024;

/// Errors returned by the client surfaces.
#[derive(Debug, Error)]
pub enum HttpClientError {
    /// The HTTP/1.0 send coroutine failed.
    #[error(transparent)]
    Http10Send(#[from] Http10SendError),
    /// The HTTP/1.1 send coroutine failed.
    #[error(transparent)]
    Http11Send(#[from] Http11SendError),
    /// The underlying stream failed to read or write.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// The TCP connection or the TLS negotiation failed.
    #[cfg(any(
        feature = "rustls-aws",
        feature = "rustls-ring",
        feature = "native-tls"
    ))]
    #[error(transparent)]
    Tls(#[from] anyhow::Error),
    /// The URL to connect to carries no host.
    #[error("HTTP URL `{0}` has no host")]
    UrlMissingHost(String),
    /// The URL to connect to carries a scheme the client cannot open.
    #[error("HTTP URL `{0}` has unsupported scheme `{1}` (expected `http` or `https`)")]
    UrlUnsupportedScheme(String, String),
    /// The server answered with a redirect the client never follows.
    #[error("HTTP server redirected to `{url}` (status `{code}`)")]
    UnexpectedRedirect {
        /// The resolved redirect target.
        url: Url,
        /// The 3xx status code of the response.
        code: u16,
    },
    /// The streaming response did not use chunked transfer coding.
    #[error("HTTP streaming requires `Transfer-Encoding: chunked` (got status `{0}`)")]
    StreamingNotChunked(u16),
    /// The streaming chunked-body decoder failed.
    #[error(transparent)]
    ChunkStream(#[from] Http11ChunksReadStreamError),
    /// The implementor's own transport failed.
    ///
    /// [`HttpClientStd`] reports I/O through [`Self::Io`]; this variant
    /// exists for implementors whose failures are something else, such
    /// as a JNI upcall or a runtime-specific socket error.
    #[error(transparent)]
    Transport(Box<dyn core::error::Error + Send + Sync>),
}

/// Blocking HTTP request surface: implement the two pumps and inherit
/// the commands.
///
/// [`HttpClientStd`] implements it over a `Read + Write` stream; a
/// caller whose transport is its own (a JNI upcall bridge, an in-memory
/// test double) implements the same two methods and gets the rest.
///
/// There are two pumps rather than one because HTTP has two yield
/// vocabularies. [`run`] takes the plain read/write coroutines, the ones
/// every client wraps identically. [`run_send`] takes the request
/// coroutines, which also yield [`HttpSendYield::WantsRedirect`], and
/// that yield is a policy question: this crate's own client refuses a
/// redirect, a browser-shaped one would follow it, and a consumer
/// bounded by an allow-list would inspect it. Making it a required
/// method puts the decision in the implementor's hands and keeps it out
/// of the defaults.
///
/// The trait is not dyn-compatible, because both pumps are generic. The
/// dynamism this crate needs lives one layer down, at the boxed stream
/// [`HttpClientStd`] holds.
///
/// [`run`]: Self::run
/// [`run_send`]: Self::run_send
pub trait HttpClient {
    /// Runs a standard-shape coroutine to completion, fulfilling its
    /// read and write requests against the transport.
    fn run<C, T, E>(&mut self, coroutine: C) -> Result<T, HttpClientError>
    where
        C: HttpCoroutine<Yield = HttpYield, Return = Result<T, E>>,
        HttpClientError: From<E>;

    /// Runs a request coroutine to completion, deciding what a redirect
    /// means along the way.
    fn run_send<C, E>(&mut self, coroutine: C) -> Result<HttpSendOutput, HttpClientError>
    where
        C: HttpCoroutine<Yield = HttpSendYield, Return = Result<HttpSendOutput, E>>,
        HttpClientError: From<E>;

    /// Sends one HTTP/1.1 request and reads its response.
    fn send(&mut self, request: HttpRequest) -> Result<HttpSendOutput, HttpClientError> {
        self.run_send(Http11Send::new(request))
    }

    /// HTTP/1.0 counterpart of [`send`](Self::send).
    fn send_http10(&mut self, request: HttpRequest) -> Result<HttpSendOutput, HttpClientError> {
        self.run_send(Http10Send::new(request))
    }
}

/// Async HTTP request surface, the [`HttpClient`] twin for callers
/// whose transport is a future.
///
/// Everything [`HttpClient`] documents applies here, plus the `Send`
/// bounds. They are load-bearing rather than defensive: a plain `async
/// fn` in a trait cannot promise that the future it returns is `Send`,
/// so anything built from the default bodies would fail to compile
/// under `tokio::spawn`, which is the first thing a worker-spawning
/// consumer reaches for. Declaring the return type explicitly as `impl
/// Future<..> + Send`, with `Send` as a supertrait so `&mut Self`
/// carries through, keeps the defaults spawnable.
///
/// [`HttpClient`] deliberately carries no such bound. A blocking call
/// returns a value, so there is no future whose auto-traits need
/// pinning down, and requiring `Send` there would exclude a perfectly
/// good client built on a thread-affine handle.
pub trait HttpClientAsync: Send {
    /// Runs a standard-shape coroutine to completion, fulfilling its
    /// read and write requests against the transport.
    fn run<C, T, E>(
        &mut self,
        coroutine: C,
    ) -> impl Future<Output = Result<T, HttpClientError>> + Send
    where
        C: HttpCoroutine<Yield = HttpYield, Return = Result<T, E>> + Send,
        T: Send,
        E: Send,
        HttpClientError: From<E>;

    /// Runs a request coroutine to completion, deciding what a redirect
    /// means along the way.
    fn run_send<C, E>(
        &mut self,
        coroutine: C,
    ) -> impl Future<Output = Result<HttpSendOutput, HttpClientError>> + Send
    where
        C: HttpCoroutine<Yield = HttpSendYield, Return = Result<HttpSendOutput, E>> + Send,
        E: Send,
        HttpClientError: From<E>;

    /// Sends one HTTP/1.1 request and reads its response.
    fn send(
        &mut self,
        request: HttpRequest,
    ) -> impl Future<Output = Result<HttpSendOutput, HttpClientError>> + Send {
        self.run_send(Http11Send::new(request))
    }

    /// HTTP/1.0 counterpart of [`send`](Self::send).
    fn send_http10(
        &mut self,
        request: HttpRequest,
    ) -> impl Future<Output = Result<HttpSendOutput, HttpClientError>> + Send {
        self.run_send(Http10Send::new(request))
    }
}

/// Std-blocking HTTP client wrapping a boxed `Read + Write + Send` stream.
pub struct HttpClientStd {
    stream: Box<dyn HttpStream>,
}

impl HttpClientStd {
    /// Wraps a pre-connected stream; caller handles TCP and TLS.
    pub fn new<S: Read + Write + Send + 'static>(stream: S) -> Self {
        Self {
            stream: Box::new(stream),
        }
    }

    /// Default ALPN identifier for HTTPS connections: `http/1.1`
    /// ([RFC 7301] + IANA registry).
    ///
    /// [RFC 7301]: https://www.rfc-editor.org/rfc/rfc7301
    pub fn default_alpn() -> Vec<String> {
        vec![String::from("http/1.1")]
    }

    /// Replaces the underlying stream (e.g. after `Connection: close` or
    /// a cross-authority redirect).
    pub fn set_stream<S: Read + Write + Send + 'static>(&mut self, stream: S) {
        self.stream = Box::new(stream);
    }
}

impl HttpClient for HttpClientStd {
    fn run<C, T, E>(&mut self, mut coroutine: C) -> Result<T, HttpClientError>
    where
        C: HttpCoroutine<Yield = HttpYield, Return = Result<T, E>>,
        HttpClientError: From<E>,
    {
        let mut buf = [0u8; READ_BUFFER_SIZE];
        let mut arg: Option<&[u8]> = None;

        loop {
            match coroutine.resume(arg.take()) {
                HttpCoroutineState::Complete(Ok(out)) => return Ok(out),
                HttpCoroutineState::Complete(Err(err)) => return Err(err.into()),
                HttpCoroutineState::Yielded(HttpYield::WantsRead) => {
                    let n = self.stream.read(&mut buf)?;
                    arg = Some(&buf[..n]);
                }
                HttpCoroutineState::Yielded(HttpYield::WantsWrite(bytes)) => {
                    self.stream.write_all(&bytes)?;
                    arg = None;
                }
            }
        }
    }

    /// Refuses a redirect with [`HttpClientError::UnexpectedRedirect`],
    /// this client following none: a request carrying credentials must
    /// not replay them against whatever host a 3xx names, and the
    /// caller is the only party that knows whether the new target is
    /// one it meant to talk to.
    fn run_send<C, E>(&mut self, mut coroutine: C) -> Result<HttpSendOutput, HttpClientError>
    where
        C: HttpCoroutine<Yield = HttpSendYield, Return = Result<HttpSendOutput, E>>,
        HttpClientError: From<E>,
    {
        let mut buf = [0u8; READ_BUFFER_SIZE];
        let mut arg: Option<&[u8]> = None;

        loop {
            match coroutine.resume(arg.take()) {
                HttpCoroutineState::Complete(Ok(out)) => return Ok(out),
                HttpCoroutineState::Complete(Err(err)) => return Err(err.into()),
                HttpCoroutineState::Yielded(HttpSendYield::WantsRead) => {
                    let n = self.stream.read(&mut buf)?;
                    arg = Some(&buf[..n]);
                }
                HttpCoroutineState::Yielded(HttpSendYield::WantsWrite(bytes)) => {
                    self.stream.write_all(&bytes)?;
                    arg = None;
                }
                HttpCoroutineState::Yielded(HttpSendYield::WantsRedirect {
                    url, response, ..
                }) => {
                    return Err(HttpClientError::UnexpectedRedirect {
                        url,
                        code: *response.status,
                    });
                }
            }
        }
    }
}

impl HttpClientStd {
    /// Opens an HTTP/1.1 SSE stream; requires `Transfer-Encoding: chunked`.
    /// Consumes `self` because the connection is dedicated to the stream.
    pub fn send_streaming(self, request: HttpRequest) -> Result<SseStream, HttpClientError> {
        let HttpClientStd { mut stream } = self;

        let req_bytes = request.to_http_11_vec();
        stream.write_all(&req_bytes)?;

        let mut read_headers = Http11HeadersRead::default();
        let mut buf = [0u8; READ_BUFFER_SIZE];
        let mut arg: Option<&[u8]> = None;

        let out = loop {
            match read_headers.resume(arg.take()) {
                HttpCoroutineState::Complete(Ok(out)) => break out,
                HttpCoroutineState::Complete(Err(err)) => {
                    return Err(Http11SendError::from(err).into());
                }
                HttpCoroutineState::Yielded(HttpYield::WantsRead) => {
                    let n = stream.read(&mut buf)?;
                    if n == 0 {
                        return Err(Http11SendError::Eof.into());
                    }
                    arg = Some(&buf[..n]);
                }
                HttpCoroutineState::Yielded(HttpYield::WantsWrite(_)) => {
                    unreachable!("Http11HeadersRead never writes");
                }
            }
        };

        let chunked = out
            .response
            .header(HTTP_TRANSFER_ENCODING)
            .is_some_and(|enc| enc.eq_ignore_ascii_case("chunked"));

        if !chunked {
            return Err(HttpClientError::StreamingNotChunked(*out.response.status));
        }

        Ok(SseStream {
            stream,
            chunk_stream: Http11ChunksReadStream::default(),
            sse_parser: SseFrameParser::default(),
            pending: None,
            preread: out.remaining,
            response: out.response,
            keep_alive: out.keep_alive,
            done: false,
        })
    }
}

/// Long-lived HTTP/1.1 Server-Sent Events stream; each
/// [`SseStream::next_frame`] / [`Iterator::next`] blocks until the next
/// event arrives or the connection closes.
pub struct SseStream {
    stream: Box<dyn HttpStream>,
    chunk_stream: Http11ChunksReadStream,
    sse_parser: SseFrameParser,
    pending: Option<Vec<u8>>,
    preread: Vec<u8>,
    response: HttpResponse,
    keep_alive: bool,
    done: bool,
}

impl SseStream {
    /// Parsed response headers (body is the streaming channel itself).
    pub fn response(&self) -> &HttpResponse {
        &self.response
    }

    /// Whether the server signalled the connection can be reused.
    pub fn keep_alive(&self) -> bool {
        self.keep_alive
    }

    /// Last-event-id seen so far; supply via `Last-Event-ID` on reconnect.
    pub fn last_event_id(&self) -> Option<&str> {
        self.sse_parser.last_event_id()
    }

    /// Drives chunked + SSE decoding until the next event; [`None`] on
    /// connection close or zero-length chunk terminator.
    pub fn next_frame(&mut self) -> Result<Option<SseFrame>, HttpClientError> {
        if self.done {
            return Ok(None);
        }

        loop {
            let arg = self.pending.take();
            match self.sse_parser.resume(arg.as_deref()) {
                HttpCoroutineState::Yielded(SseFrameParserYield::Frame(frame)) => {
                    return Ok(Some(frame));
                }
                HttpCoroutineState::Yielded(SseFrameParserYield::WantsBytes) => {
                    match self.pull_chunk()? {
                        Some(body) => self.pending = Some(body),
                        None => {
                            self.done = true;
                            return Ok(None);
                        }
                    }
                }
                HttpCoroutineState::Complete(never) => match never {},
            }
        }
    }

    /// Closes the underlying connection (equivalent to dropping `self`).
    pub fn close(self) {
        drop(self);
    }

    fn pull_chunk(&mut self) -> Result<Option<Vec<u8>>, HttpClientError> {
        let mut tmp = [0u8; READ_BUFFER_SIZE];
        let preread = mem::take(&mut self.preread);
        let mut arg: Option<&[u8]> = if preread.is_empty() {
            None
        } else {
            Some(&preread)
        };

        loop {
            match self.chunk_stream.resume(arg.take()) {
                HttpCoroutineState::Yielded(Http11ChunksReadStreamYield::Frame { body }) => {
                    return Ok(Some(body));
                }
                HttpCoroutineState::Complete(Ok(_remaining)) => return Ok(None),
                HttpCoroutineState::Yielded(Http11ChunksReadStreamYield::WantsRead) => {
                    let n = self.stream.read(&mut tmp)?;
                    if n == 0 {
                        return Ok(None);
                    }
                    arg = Some(&tmp[..n]);
                }
                HttpCoroutineState::Complete(Err(err)) => return Err(err.into()),
            }
        }
    }
}

impl Iterator for SseStream {
    type Item = Result<SseFrame, HttpClientError>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.next_frame() {
            Ok(Some(frame)) => Some(Ok(frame)),
            Ok(None) => None,
            Err(err) => Some(Err(err)),
        }
    }
}

/// Marker for everything the client can run against; the `Send`
/// supertrait propagates through the `Box<dyn HttpStream>` erasure so
/// [`HttpClientStd`] stays `Send`.
trait HttpStream: Read + Write + Send {}
impl<T: Read + Write + Send + ?Sized> HttpStream for T {}
