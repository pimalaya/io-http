//! I/O-free coroutine to discover a service endpoint via the
//! `.well-known` URI scheme (RFC 8615).
//!
//! The discovery flow is a single HTTP exchange:
//!
//! 1. Client sends `GET /.well-known/{service}` to the origin.
//! 2. Server responds — surfaced as [`WellKnownResult::Ok`].
//!    Inspect `redirect_url` to know whether the server redirected
//!    (the expected case) or responded directly.
//!
//! Use [`WellKnown::prepare_request`] to build the request, then
//! drive the coroutine with [`WellKnown::resume`]:
//!
//! ```rust,ignore
//! use std::{io::{Read, Write}, net::TcpStream};
//! use io_http::rfc8615::well_known::{WellKnown, WellKnownResult};
//!
//! let request = WellKnown::prepare_request("http://example.com", "caldav").unwrap();
//! let mut stream = TcpStream::connect("example.com:80").unwrap();
//! let mut well_known = WellKnown::new(request);
//! let mut arg: Option<&[u8]> = None;
//! let mut buf = [0u8; 4096];
//!
//! loop {
//!     match well_known.resume(arg.take()) {
//!         WellKnownResult::Ok { redirect_url: Some(url), .. } => {
//!             println!("caldav endpoint: {url}");
//!             break;
//!         }
//!         WellKnownResult::Ok { response, .. } => {
//!             panic!("expected redirect, got {}", *response.status);
//!         }
//!         WellKnownResult::Err(err) => panic!("{err}"),
//!         WellKnownResult::WantsRead => {
//!             let n = stream.read(&mut buf).unwrap();
//!             arg = Some(&buf[..n]);
//!         }
//!         WellKnownResult::WantsWrite(bytes) => {
//!             stream.write_all(&bytes).unwrap();
//!         }
//!     }
//! }
//! ```

use alloc::{format, string::String, vec::Vec};

use thiserror::Error;
use url::{ParseError, Url};

use crate::{
    rfc9110::{request::HttpRequest, response::HttpResponse},
    rfc9112::send::{Http11Send, Http11SendError, Http11SendResult},
};

/// Errors that can occur during the coroutine progression.
#[derive(Debug, Error)]
pub enum WellKnownError {
    #[error("Invalid base URL {1}")]
    InvalidBaseUrl(#[source] ParseError, String),
    #[error(transparent)]
    Send(#[from] Http11SendError),
}

/// Result returned by [`WellKnown::resume`].
#[derive(Debug)]
pub enum WellKnownResult {
    /// The coroutine has successfully terminated its execution.
    Ok {
        /// The response received.
        response: HttpResponse,
        /// Whether the server indicated the connection can be reused.
        keep_alive: bool,
        /// Whether the response stayed on the same scheme, host, and
        /// port as the request.
        ///
        /// Always `true` for non-redirect responses. When `false` on
        /// a redirect, forwarding credentials to the new host without
        /// user consent is inadvisable (RFC 9110 §15.4).
        same_origin: bool,
        /// The resolved redirect target URL, if the server responded
        /// with a 3xx and a parseable `Location` header.
        ///
        /// `None` when the server responded directly (non-redirect).
        redirect_url: Option<Url>,
    },

    /// The coroutine needs more bytes to be read from the socket.
    WantsRead,

    /// The coroutine wants the given bytes to be written to the
    /// socket.
    WantsWrite(Vec<u8>),

    /// The coroutine encountered an error.
    Err(WellKnownError),
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

    /// Advances the coroutine.
    ///
    /// Pass [`None`] when there is no data to provide (initial call,
    /// after a write). Pass `Some(data)` with bytes read from the
    /// socket after a [`WellKnownResult::WantsRead`]. Pass `Some(&[])`
    /// to signal EOF.
    pub fn resume(&mut self, arg: Option<&[u8]>) -> WellKnownResult {
        match self.0.resume(arg) {
            Http11SendResult::Ok {
                response,
                keep_alive,
                ..
            } => WellKnownResult::Ok {
                response,
                keep_alive,
                same_origin: true,
                redirect_url: None,
            },
            Http11SendResult::WantsRead => WellKnownResult::WantsRead,
            Http11SendResult::WantsWrite(bytes) => WellKnownResult::WantsWrite(bytes),
            Http11SendResult::WantsRedirect {
                url,
                response,
                keep_alive,
                same_origin,
            } => WellKnownResult::Ok {
                response,
                keep_alive,
                same_origin,
                redirect_url: Some(url),
            },
            Http11SendResult::Err(err) => WellKnownResult::Err(err.into()),
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
