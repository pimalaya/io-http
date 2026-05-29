//! I/O-free coroutine to discover a service endpoint via the
//! `.well-known` URI scheme (RFC 8615).
//!
//! The discovery flow is a single HTTP exchange:
//!
//! 1. Client sends `GET /.well-known/{service}` to the origin.
//! 2. Server responds. Inspect `redirect_url` on the
//!    [`WellKnownOutput`] to know whether the server redirected (the
//!    expected case) or responded directly.
//!
//! Use [`WellKnown::prepare_request`] to build the request, then drive
//! the coroutine with [`WellKnown::resume`]:
//!
//! ```rust,ignore
//! use std::{io::{Read, Write}, net::TcpStream};
//! use io_http::{coroutine::*, rfc8615::well_known::WellKnown};
//!
//! let request = WellKnown::prepare_request("http://example.com", "caldav").unwrap();
//! let mut stream = TcpStream::connect("example.com:80").unwrap();
//! let mut well_known = WellKnown::new(request);
//! let mut arg: Option<&[u8]> = None;
//! let mut buf = [0u8; 4096];
//!
//! loop {
//!     match well_known.resume(arg.take()) {
//!         HttpCoroutineState::Complete(Ok(out)) if out.redirect_url.is_some() => {
//!             println!("caldav endpoint: {}", out.redirect_url.unwrap());
//!             break;
//!         }
//!         HttpCoroutineState::Complete(Ok(out)) => {
//!             panic!("expected redirect, got {}", *out.response.status);
//!         }
//!         HttpCoroutineState::Complete(Err(err)) => panic!("{err}"),
//!         HttpCoroutineState::Yielded(HttpYield::WantsRead) => {
//!             let n = stream.read(&mut buf).unwrap();
//!             arg = Some(&buf[..n]);
//!         }
//!         HttpCoroutineState::Yielded(HttpYield::WantsWrite(bytes)) => {
//!             stream.write_all(&bytes).unwrap();
//!         }
//!     }
//! }
//! ```

use alloc::{format, string::String};

use thiserror::Error;
use url::{ParseError, Url};

use crate::{
    coroutine::*,
    rfc9110::{request::HttpRequest, response::HttpResponse, send::HttpSendYield},
    rfc9112::send::{Http11Send, Http11SendError},
};

/// Errors that can occur during the coroutine progression.
#[derive(Debug, Error)]
pub enum WellKnownError {
    #[error("Invalid base URL {1}")]
    InvalidBaseUrl(#[source] ParseError, String),
    #[error(transparent)]
    Send(#[from] Http11SendError),
}

/// Terminal output of [`WellKnown`].
#[derive(Debug)]
pub struct WellKnownOutput {
    /// The response received.
    pub response: HttpResponse,
    /// Whether the server indicated the connection can be reused.
    pub keep_alive: bool,
    /// Whether the response stayed on the same scheme, host, and port
    /// as the request.
    ///
    /// Always `true` for non-redirect responses. When `false` on a
    /// redirect, forwarding credentials to the new host without user
    /// consent is inadvisable (RFC 9110 §15.4).
    pub same_origin: bool,
    /// The resolved redirect target URL, if the server responded with
    /// a 3xx and a parseable `Location` header.
    ///
    /// `None` when the server responded directly (non-redirect).
    pub redirect_url: Option<Url>,
}

/// I/O-free coroutine to perform a `.well-known` URI discovery request.
#[derive(Debug)]
pub struct WellKnown(Http11Send);

impl WellKnown {
    /// Builds a GET request for `/.well-known/{service}` on the given
    /// base URL.
    ///
    /// The base URL's scheme, host, and port are preserved; only the
    /// path is replaced with `/.well-known/{service}`.
    ///
    /// # Errors
    ///
    /// Returns [`WellKnownError::InvalidBaseUrl`] if `base_url` cannot
    /// be parsed as an absolute URL.
    pub fn prepare_request(
        base_url: impl AsRef<str>,
        service: impl AsRef<str>,
    ) -> Result<HttpRequest, WellKnownError> {
        let base = base_url.as_ref();
        let mut url =
            Url::parse(base).map_err(|e| WellKnownError::InvalidBaseUrl(e, base.into()))?;
        url.set_path(&format!("/.well-known/{}", service.as_ref()));
        Ok(HttpRequest::get(url))
    }

    /// Creates a new coroutine from the given request.
    ///
    /// Use [`WellKnown::prepare_request`] to build a correctly-formed
    /// request, or supply a custom [`HttpRequest`] directly.
    pub fn new(request: HttpRequest) -> Self {
        Self(Http11Send::new(request))
    }
}

impl HttpCoroutine for WellKnown {
    type Yield = HttpYield;
    type Return = Result<WellKnownOutput, WellKnownError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> HttpCoroutineState<Self::Yield, Self::Return> {
        match self.0.resume(arg) {
            HttpCoroutineState::Complete(Ok(out)) => {
                HttpCoroutineState::Complete(Ok(WellKnownOutput {
                    response: out.response,
                    keep_alive: out.keep_alive,
                    same_origin: true,
                    redirect_url: None,
                }))
            }
            HttpCoroutineState::Yielded(HttpSendYield::WantsRead) => {
                HttpCoroutineState::Yielded(HttpYield::WantsRead)
            }
            HttpCoroutineState::Yielded(HttpSendYield::WantsWrite(bytes)) => {
                HttpCoroutineState::Yielded(HttpYield::WantsWrite(bytes))
            }
            HttpCoroutineState::Yielded(HttpSendYield::WantsRedirect {
                url,
                response,
                keep_alive,
                same_origin,
            }) => HttpCoroutineState::Complete(Ok(WellKnownOutput {
                response,
                keep_alive,
                same_origin,
                redirect_url: Some(url),
            })),
            HttpCoroutineState::Complete(Err(err)) => HttpCoroutineState::Complete(Err(err.into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rfc8615::well_known::*;

    #[test]
    fn prepare_request_sets_well_known_path() {
        let req = WellKnown::prepare_request("http://example.com", "caldav").unwrap();
        assert_eq!(req.url.path(), "/.well-known/caldav");
    }

    #[test]
    fn prepare_request_preserves_scheme_and_host() {
        let req = WellKnown::prepare_request("https://example.com", "carddav").unwrap();
        assert_eq!(req.url.scheme(), "https");
        assert_eq!(req.url.host_str(), Some("example.com"));
    }

    #[test]
    fn prepare_request_preserves_port() {
        let req = WellKnown::prepare_request("http://example.com:8080", "oauth").unwrap();
        assert_eq!(req.url.port(), Some(8080));
    }

    #[test]
    fn prepare_request_rejects_invalid_url() {
        let result = WellKnown::prepare_request("not a url", "caldav");
        assert!(result.is_err(), "expected Err for an invalid base URL");
    }
}
