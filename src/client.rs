//! # Standard, blocking HTTP/1.X client
//!
//! Holds a single boxed [`HttpStream`] (any blocking `Read + Write`
//! impl) and exposes one method per common coroutine. HTTP has no
//! long-lived session context — each [`send`] / [`send_http10`] is
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
    rfc1945::send::*,
    rfc9110::{request::HttpRequest, response::HttpResponse},
    rfc9112::send::*,
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
}

/// Output of [`HttpClientStd::send`] / [`HttpClientStd::send_http10`].
#[derive(Clone, Debug)]
pub struct HttpSendOutput {
    /// The parsed HTTP response.
    pub response: HttpResponse,
    /// Bytes pre-read past the response body — the caller should feed
    /// these to the next coroutine.
    pub remaining: Vec<u8>,
    /// Whether the server indicated the connection can be reused.
    pub keep_alive: bool,
}

/// Std-blocking HTTP client wrapping a single [`HttpStream`].
pub struct HttpClientStd {
    stream: Box<dyn Stream>,
}

impl HttpClientStd {
    /// Builds a client around `stream`. The caller is responsible
    /// for opening the connection (TCP, TLS handshake if needed).
    pub fn new<S: Read + Write + 'static>(stream: S) -> Self {
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
    pub fn set_stream<S: Read + Write + 'static>(&mut self, stream: S) {
        self.stream = Box::new(stream);
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
            match coroutine.resume(arg) {
                Http11SendResult::Ok {
                    response,
                    remaining,
                    keep_alive,
                } => {
                    return Ok(HttpSendOutput {
                        response,
                        remaining,
                        keep_alive,
                    });
                }
                Http11SendResult::WantsRead => {
                    let n = self.stream.read(&mut buf)?;
                    arg = Some(&buf[..n]);
                }
                Http11SendResult::WantsWrite(bytes) => {
                    self.stream.write_all(&bytes)?;
                    arg = None;
                }
                Http11SendResult::WantsRedirect { url, response, .. } => {
                    return Err(HttpClientStdError::UnexpectedRedirect {
                        url,
                        code: *response.status,
                    });
                }
                Http11SendResult::Err(err) => return Err(err.into()),
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
            match coroutine.resume(arg) {
                Http10SendResult::Ok {
                    response,
                    remaining,
                    keep_alive,
                } => {
                    return Ok(HttpSendOutput {
                        response,
                        remaining,
                        keep_alive,
                    });
                }
                Http10SendResult::WantsRead => {
                    let n = self.stream.read(&mut buf)?;
                    arg = Some(&buf[..n]);
                }
                Http10SendResult::WantsWrite(bytes) => {
                    self.stream.write_all(&bytes)?;
                    arg = None;
                }
                Http10SendResult::WantsRedirect { url, response, .. } => {
                    return Err(HttpClientStdError::UnexpectedRedirect {
                        url,
                        code: *response.status,
                    });
                }
                Http10SendResult::Err(err) => return Err(err.into()),
            }
        }
    }
}

/// Marker for everything the client can run against; auto-implemented
/// for any blocking `Read + Write` impl.
trait Stream: Read + Write {}
impl<T: Read + Write + ?Sized> Stream for T {}
