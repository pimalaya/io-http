//! Standard, blocking HTTP/1.X client wrapping a boxed
//! `Read + Write + Send` stream. Each [`HttpClientStd::send`] /
//! [`HttpClientStd::send_http10`] is self-contained (HTTP has no
//! session context). With a TLS feature enabled,
//! [`HttpClientStd::connect`] opens `http://` / `https://` URLs
//! end-to-end via [`pimalaya_stream::std::stream::StreamStd`].

#[cfg(any(
    feature = "rustls-aws",
    feature = "rustls-ring",
    feature = "native-tls"
))]
use alloc::string::ToString;
use alloc::{boxed::Box, string::String, vec, vec::Vec};

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

/// Default ALPN identifier for HTTPS connections: `http/1.1`
/// ([RFC 7301] + IANA registry).
///
/// [RFC 7301]: https://www.rfc-editor.org/rfc/rfc7301
pub fn default_alpn() -> Vec<String> {
    vec![String::from("http/1.1")]
}

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

    /// Connects to `url` (TLS handshake on `https`), reading ALPN from
    /// `tls.rustls.alpn` (see [`default_alpn`]).
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

    /// Replaces the underlying stream (e.g. after `Connection: close` or
    /// a cross-authority redirect).
    pub fn set_stream<S: Read + Write + Send + 'static>(&mut self, stream: S) {
        self.stream = Box::new(stream);
    }

    /// Drives any standard-shape coroutine against the wrapped stream
    /// until it completes. Coroutines with richer Yield variants
    /// (`Http*Send`, `Http11ReadChunksStream`, `SseFrameParser`) use
    /// their own per-method loops below.
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

    /// Runs [`Http11Send`]; surfaces 3xx as
    /// [`HttpClientStdError::UnexpectedRedirect`].
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

    /// HTTP/1.0 counterpart of [`Self::send`].
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
    /// Opens an HTTP/1.1 SSE stream; requires `Transfer-Encoding: chunked`.
    /// Consumes `self` because the connection is dedicated to the stream.
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

/// Long-lived HTTP/1.1 Server-Sent Events stream; each
/// [`SseStream::next_frame`] / [`Iterator::next`] blocks until the next
/// event arrives or the connection closes.
pub struct SseStream {
    stream: Box<dyn HttpStream>,
    chunk_stream: Http11ReadChunksStream,
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

    /// Closes the underlying connection (equivalent to dropping `self`).
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

// Marker for everything the client can run against; the `Send`
// supertrait propagates through the `Box<dyn HttpStream>` erasure so
// `HttpClientStd` stays `Send`.
trait HttpStream: Read + Write + Send {}
impl<T: Read + Write + Send + ?Sized> HttpStream for T {}
