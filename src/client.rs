//! # Standard, blocking HTTP/1.X client
//!
//! Holds a single boxed `Read + Write + Send` stream and exposes one
//! method per common coroutine. HTTP has no
//! long-lived session context: each [`send`] / [`send_http10`] is
//! self-contained.
//!
//! The bare [`new`] constructor takes a pre-connected stream;
//! callers handle TCP and TLS themselves. With one of the TLS feature
//! flags enabled (`rustls-ring`, `rustls-aws`, `native-tls`),
//! [`connect`] is also available and handles `http://` / `https://`
//! URLs end-to-end via [`pimalaya_stream::std::stream::StreamStd`].
//!
//! [`new`]: HttpClientStd::new
//! [`connect`]: HttpClientStd::connect
//! [`send`]: HttpClientStd::send
//! [`send_http10`]: HttpClientStd::send_http10

#[cfg(any(
    feature = "rustls-aws",
    feature = "rustls-ring",
    feature = "native-tls"
))]
use alloc::string::{String, ToString};
use alloc::{boxed::Box, vec::Vec};
use std::io::{self, Read, Write};

#[cfg(any(
    feature = "rustls-aws",
    feature = "rustls-ring",
    feature = "native-tls"
))]
use pimalaya_stream::{std::stream::StreamStd, tls::Tls};
use thiserror::Error;
use url::Url;

use crate::{
    coroutine::*,
    rfc1945::send::*,
    rfc9110::{
        headers::TRANSFER_ENCODING,
        request::HttpRequest,
        response::HttpResponse,
        send::{HttpSendOutput, HttpSendYield},
    },
    rfc9112::{chunk_stream::*, read_headers::*, send::*},
    sse::frame::*,
};

const READ_BUFFER_SIZE: usize = 16 * 1024;

/// Errors returned by [`HttpClientStd`].
#[derive(Debug, Error)]
pub enum HttpClientStdError {
    #[error(transparent)]
    Http10Send(#[from] Http10SendError),
    #[error(transparent)]
    Http11Send(#[from] Http11SendError),

    #[error(transparent)]
    Io(#[from] io::Error),

    #[cfg(any(
        feature = "rustls-aws",
        feature = "rustls-ring",
        feature = "native-tls"
    ))]
    #[error(transparent)]
    Tls(#[from] anyhow::Error),
    #[cfg(any(
        feature = "rustls-aws",
        feature = "rustls-ring",
        feature = "native-tls"
    ))]
    #[error("HTTP URL `{0}` has no host")]
    UrlMissingHost(String),
    #[cfg(any(
        feature = "rustls-aws",
        feature = "rustls-ring",
        feature = "native-tls"
    ))]
    #[error("HTTP URL `{0}` has unsupported scheme `{1}` (expected `http` or `https`)")]
    UrlUnsupportedScheme(String, String),

    #[error("HTTP server redirected to `{url}` (status `{code}`)")]
    UnexpectedRedirect { url: Url, code: u16 },

    #[error("HTTP streaming requires `Transfer-Encoding: chunked` (got status `{0}`)")]
    StreamingNotChunked(u16),
    #[error(transparent)]
    ChunkStream(#[from] Http11ReadChunksStreamError),
}

/// Std-blocking HTTP client wrapping a single boxed `Read + Write +
/// Send` stream.
pub struct HttpClientStd {
    stream: Box<dyn Stream>,
}

impl HttpClientStd {
    /// Builds a client around `stream`. The caller is responsible
    /// for opening the connection (TCP, TLS handshake if needed).
    pub fn new<S: Read + Write + Send + 'static>(stream: S) -> Self {
        Self {
            stream: Box::new(stream),
        }
    }

    /// Connects to `url` and runs the TLS handshake when the scheme
    /// is `https`. `http` URLs go through plain TCP. ALPN is set to
    /// `http/1.1`.
    #[cfg(any(
        feature = "rustls-aws",
        feature = "rustls-ring",
        feature = "native-tls"
    ))]
    pub fn connect(url: &Url, tls: &Tls) -> Result<Self, HttpClientStdError> {
        let host = url
            .host_str()
            .ok_or_else(|| HttpClientStdError::UrlMissingHost(url.to_string()))?;

        let stream = match url.scheme() {
            "http" => StreamStd::connect_tcp(host, url.port_or_known_default().unwrap_or(80))?,
            "https" => {
                StreamStd::connect_tls(host, url.port_or_known_default().unwrap_or(443), tls)?
            }
            scheme => {
                return Err(HttpClientStdError::UrlUnsupportedScheme(
                    url.to_string(),
                    scheme.to_string(),
                ));
            }
        };

        Ok(Self {
            stream: Box::new(stream),
        })
    }

    /// Replaces the underlying stream — useful when the server
    /// signals `Connection: close` or redirects to a different
    /// authority and a fresh transport must be opened.
    pub fn set_stream<S: Read + Write + Send + 'static>(&mut self, stream: S) {
        self.stream = Box::new(stream);
    }

    /// Drives any standard-shape coroutine (`Yield = HttpYield`,
    /// `Return = Result<Output, Error>`) against the wrapped stream
    /// until it terminates.
    ///
    /// Coroutines that need richer Yield variants (e.g. [`Http11Send`]
    /// with [`HttpSendYield::WantsRedirect`], or
    /// [`Http11ReadChunksStream`] / [`SseFrameParser`] for streaming)
    /// are driven by their own per-method loops on this client; see
    /// [`Self::send`], [`Self::send_streaming`], and [`SseStream`].
    pub fn run<C, T, E>(&mut self, mut coroutine: C) -> Result<T, HttpClientStdError>
    where
        C: HttpCoroutine<Yield = HttpYield, Return = Result<T, E>>,
        HttpClientStdError: From<E>,
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

    /// Runs [`Http11Send`] (RFC 9112): sends `request` over the
    /// underlying stream and reads back the response. Returns
    /// [`HttpClientStdError::UnexpectedRedirect`] on 3xx; the caller
    /// can inspect the URL and retry against a new client.
    pub fn send(&mut self, request: HttpRequest) -> Result<HttpSendOutput, HttpClientStdError> {
        let mut coroutine = Http11Send::new(request);
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
                    return Err(HttpClientStdError::UnexpectedRedirect {
                        url,
                        code: *response.status,
                    });
                }
            }
        }
    }

    /// Runs [`Http10Send`] (RFC 1945): same as [`send`] but speaks
    /// HTTP/1.0. Use this only when targeting a server that does not
    /// support HTTP/1.1.
    ///
    /// [`send`]: HttpClientStd::send
    pub fn send_http10(
        &mut self,
        request: HttpRequest,
    ) -> Result<HttpSendOutput, HttpClientStdError> {
        let mut coroutine = Http10Send::new(request);
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
                    return Err(HttpClientStdError::UnexpectedRedirect {
                        url,
                        code: *response.status,
                    });
                }
            }
        }
    }
}

impl HttpClientStd {
    /// Opens a streaming HTTP/1.1 response for [Server-Sent Events]:
    /// writes `request`, reads response headers, requires
    /// `Transfer-Encoding: chunked`, and hands the body off to the
    /// returned [`SseStream`] iterator.
    ///
    /// Consumes `self` because the underlying connection becomes
    /// dedicated to the streaming response and cannot be reused for
    /// pipelined requests. The caller drops or closes the stream when
    /// done. Pre-read body bytes (received in the same socket read as
    /// the headers) are forwarded to the stream automatically.
    ///
    /// [Server-Sent Events]: https://html.spec.whatwg.org/multipage/server-sent-events.html
    pub fn send_streaming(self, request: HttpRequest) -> Result<SseStream, HttpClientStdError> {
        let HttpClientStd { mut stream } = self;

        let req_bytes = request.to_http_11_vec();
        stream.write_all(&req_bytes)?;

        let mut read_headers = Http11ReadHeaders::default();
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
                    unreachable!("Http11ReadHeaders never writes");
                }
            }
        };

        let chunked = out
            .response
            .header(TRANSFER_ENCODING)
            .is_some_and(|enc| enc.eq_ignore_ascii_case("chunked"));

        if !chunked {
            return Err(HttpClientStdError::StreamingNotChunked(
                *out.response.status,
            ));
        }

        Ok(SseStream {
            stream,
            chunk_stream: Http11ReadChunksStream::default(),
            sse_parser: SseFrameParser::default(),
            pending: None,
            preread: out.remaining,
            response: out.response,
            keep_alive: out.keep_alive,
            done: false,
        })
    }
}

/// Long-lived streaming HTTP/1.1 response carrying a Server-Sent
/// Events body. Each call to [`SseStream::next_frame`] (or
/// [`Iterator::next`]) drives the chunked-transfer decoder and the
/// SSE frame parser, blocking on socket reads until the next event
/// arrives or the connection closes.
pub struct SseStream {
    stream: Box<dyn Stream>,
    chunk_stream: Http11ReadChunksStream,
    sse_parser: SseFrameParser,
    pending: Option<Vec<u8>>,
    preread: Vec<u8>,
    response: HttpResponse,
    keep_alive: bool,
    done: bool,
}

impl SseStream {
    /// Returns the parsed response headers (body is empty since the
    /// body is the streaming SSE channel itself).
    pub fn response(&self) -> &HttpResponse {
        &self.response
    }

    /// Whether the server signalled the connection can be reused
    /// after the stream ends. SSE servers typically use
    /// `Connection: keep-alive` and only close on shutdown.
    pub fn keep_alive(&self) -> bool {
        self.keep_alive
    }

    /// Returns the last-event-id seen on the stream so far, ready to
    /// be supplied via `Last-Event-ID` on a reconnection attempt.
    pub fn last_event_id(&self) -> Option<&str> {
        self.sse_parser.last_event_id()
    }

    /// Drives the chunked decoder and SSE frame parser until the next
    /// event is dispatched, blocking on socket reads. Returns
    /// [`None`] once the server closes the connection or sends the
    /// zero-length chunk terminator.
    pub fn next_frame(&mut self) -> Result<Option<SseFrame>, HttpClientStdError> {
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

    /// Closes the underlying connection. Equivalent to dropping the
    /// stream; provided for explicit shutdown at the call site.
    pub fn close(self) {
        drop(self);
    }

    fn pull_chunk(&mut self) -> Result<Option<Vec<u8>>, HttpClientStdError> {
        let mut tmp = [0u8; READ_BUFFER_SIZE];
        let preread = core::mem::take(&mut self.preread);
        let mut arg: Option<&[u8]> = if preread.is_empty() {
            None
        } else {
            Some(&preread)
        };

        loop {
            match self.chunk_stream.resume(arg.take()) {
                HttpCoroutineState::Yielded(Http11ReadChunksStreamYield::Frame { body }) => {
                    return Ok(Some(body));
                }
                HttpCoroutineState::Complete(Ok(_remaining)) => return Ok(None),
                HttpCoroutineState::Yielded(Http11ReadChunksStreamYield::WantsRead) => {
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
    type Item = Result<SseFrame, HttpClientStdError>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.next_frame() {
            Ok(Some(frame)) => Some(Ok(frame)),
            Ok(None) => None,
            Err(err) => Some(Err(err)),
        }
    }
}

/// Marker for everything the client can run against; auto-implemented
/// for any blocking `Read + Write + Send` impl. The `Send` supertrait
/// flows the auto-trait through the `Box<dyn Stream>` type erasure so
/// `HttpClientStd` can travel between threads. Every concrete stream
/// the pimalaya stack hands in is already `Send`.
trait Stream: Read + Write + Send {}
impl<T: Read + Write + Send + ?Sized> Stream for T {}
