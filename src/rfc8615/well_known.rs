//! I/O-free coroutine performing a `.well-known` URI discovery request
//! ([RFC 8615]).
//!
//! Wraps [`Http11Send`] and surfaces the resolved redirect URL (when
//! the server returned a 3xx with a parseable `Location`) as part of
//! the terminal output. Use [`WellKnown::prepare_request`] to build a
//! correctly-shaped GET on `/.well-known/{service}`.
//!
//! [RFC 8615]: https://www.rfc-editor.org/rfc/rfc8615

use alloc::{format, string::String};

use thiserror::Error;
use url::{ParseError, Url};

use crate::{
    coroutine::*,
    rfc9110::{request::HttpRequest, response::HttpResponse, send::HttpSendYield},
    rfc9112::send::{Http11Send, Http11SendError},
};

/// Failure causes during the HTTP well-known discovery flow.
#[derive(Debug, Error)]
pub enum WellKnownError {
    #[error("HTTP well-known failed: invalid base URL `{1}`")]
    InvalidBaseUrl(#[source] ParseError, String),
    #[error("HTTP well-known failed: {0}")]
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
    use alloc::vec::Vec;

    use super::*;

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
        let err = WellKnown::prepare_request("not a url", "caldav").unwrap_err();
        let WellKnownError::InvalidBaseUrl(_, base) = err else {
            panic!("expected InvalidBaseUrl, got {err:?}");
        };
        assert_eq!(base, "not a url");
    }

    #[test]
    fn redirect_surfaces_redirect_url() {
        let req = WellKnown::prepare_request("http://example.com", "caldav").unwrap();
        let mut coroutine = WellKnown::new(req);

        let _bytes = expect_wants_write(&mut coroutine, None);
        expect_wants_read(&mut coroutine, None);

        let reply =
            b"HTTP/1.1 301 Moved Permanently\r\nLocation: /caldav\r\nContent-Length: 0\r\n\r\n";
        let out = expect_complete_ok(&mut coroutine, Some(reply));
        let url = out.redirect_url.expect("redirect URL should be set");
        assert_eq!(url.path(), "/caldav");
        assert!(out.same_origin);
    }

    #[test]
    fn non_redirect_completes_without_redirect_url() {
        let req = WellKnown::prepare_request("http://example.com", "caldav").unwrap();
        let mut coroutine = WellKnown::new(req);

        expect_wants_write(&mut coroutine, None);
        expect_wants_read(&mut coroutine, None);

        let reply = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
        let out = expect_complete_ok(&mut coroutine, Some(reply));
        assert!(out.redirect_url.is_none());
        assert_eq!(*out.response.status, 200);
    }

    #[test]
    fn parse_error_propagates_as_send_failure() {
        let req = WellKnown::prepare_request("http://example.com", "caldav").unwrap();
        let mut coroutine = WellKnown::new(req);

        expect_wants_write(&mut coroutine, None);
        expect_wants_read(&mut coroutine, None);

        let reply = b"HTTP/1.1 200 OK\r\nContent-Length: notanumber\r\n\r\n";
        let err = expect_complete_err(&mut coroutine, Some(reply));
        assert!(
            matches!(
                err,
                WellKnownError::Send(Http11SendError::InvalidContentLength(_))
            ),
            "expected Send(InvalidContentLength), got {err:?}",
        );
    }

    // --- utils

    fn expect_wants_write(cor: &mut WellKnown, arg: Option<&[u8]>) -> Vec<u8> {
        match cor.resume(arg) {
            HttpCoroutineState::Yielded(HttpYield::WantsWrite(bytes)) => bytes,
            state => panic!("expected WantsWrite, got {state:?}"),
        }
    }

    fn expect_wants_read(cor: &mut WellKnown, arg: Option<&[u8]>) {
        match cor.resume(arg) {
            HttpCoroutineState::Yielded(HttpYield::WantsRead) => {}
            state => panic!("expected WantsRead, got {state:?}"),
        }
    }

    fn expect_complete_ok(cor: &mut WellKnown, arg: Option<&[u8]>) -> WellKnownOutput {
        match cor.resume(arg) {
            HttpCoroutineState::Complete(Ok(out)) => out,
            state => panic!("expected Complete(Ok), got {state:?}"),
        }
    }

    fn expect_complete_err(cor: &mut WellKnown, arg: Option<&[u8]>) -> WellKnownError {
        match cor.resume(arg) {
            HttpCoroutineState::Complete(Err(err)) => err,
            state => panic!("expected Complete(Err), got {state:?}"),
        }
    }
}
