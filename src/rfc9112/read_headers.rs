//! I/O-free coroutine to read and parse an HTTP/1.X response head
//! ([RFC 9112 §6]).
//!
//! Accumulates bytes until the `\r\n\r\n` head terminator, parses the
//! status line + headers via `httparse`, and emits an [`HttpResponse`]
//! with an empty body. Pre-read body bytes are returned in `remaining`
//! so the caller can hand them off to a body-decoding coroutine
//! (chunked, content-length, read-to-EOF, SSE...). `keep_alive`
//! defaults by version when `Connection` is absent (HTTP/1.0 → `false`,
//! HTTP/1.1 → `true`).
//!
//! Composed by [`super::send::Http11Send`] /
//! [`crate::rfc1945::send::Http10Send`] and reused by the SSE bootstrap
//! in [`crate::client::HttpClientStd::send_streaming`].
//!
//! [RFC 9112 §6]: https://www.rfc-editor.org/rfc/rfc9112#section-6

use alloc::vec::Vec;

use httparse::{EMPTY_HEADER, Error as HttparseError, Response, Status};
use log::trace;
use thiserror::Error;

use crate::{
    coroutine::*,
    rfc1945::version::HTTP_10,
    rfc9110::{
        headers::CONNECTION,
        response::{HttpResponse, ResponseBuilder},
        status::StatusCode,
    },
    rfc9112::version::HTTP_11,
};

/// Failure causes during the HTTP/1.X read-headers flow.
#[derive(Debug, Error)]
pub enum Http11ReadHeadersError {
    #[error("HTTP/1.X read headers failed: reached unexpected EOF before headers were complete")]
    Eof,
    #[error("HTTP/1.X read headers failed: parse response headers: {0}")]
    ParseResponseHeaders(HttparseError),
}

/// Terminal output of [`Http11ReadHeaders`].
#[derive(Debug)]
pub struct Http11ReadHeadersOutput {
    /// Parsed status line + headers; `body` is empty.
    pub response: HttpResponse,
    /// Bytes pre-read past the head terminator. Feed these to the
    /// next coroutine in the pipeline (or finalize an SSE stream
    /// using them as the initial buffered chunk).
    pub remaining: Vec<u8>,
    /// Whether the server indicated the connection can be reused.
    pub keep_alive: bool,
}

/// I/O-free coroutine to read and parse an HTTP/1.X response head.
#[derive(Debug, Default)]
pub struct Http11ReadHeaders {
    buf: Vec<u8>,
}

impl HttpCoroutine for Http11ReadHeaders {
    type Yield = HttpYield;
    type Return = Result<Http11ReadHeadersOutput, Http11ReadHeadersError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> HttpCoroutineState<Self::Yield, Self::Return> {
        match arg {
            Some(&[]) => {
                return HttpCoroutineState::Complete(Err(Http11ReadHeadersError::Eof));
            }
            Some(data) => self.buf.extend_from_slice(data),
            None => {}
        }

        let mut headers = [EMPTY_HEADER; 64];
        let mut parsed = Response::new(&mut headers);

        let header_end = match parsed.parse(&self.buf) {
            Ok(Status::Complete(n)) => n,
            Ok(Status::Partial) => {
                trace!("received incomplete headers");
                return HttpCoroutineState::Yielded(HttpYield::WantsRead);
            }
            Err(err) => {
                return HttpCoroutineState::Complete(Err(
                    Http11ReadHeadersError::ParseResponseHeaders(err),
                ));
            }
        };

        let mut builder = ResponseBuilder::default();
        let is_http10 = matches!(parsed.version, Some(0));
        builder.version = if is_http10 { HTTP_10 } else { HTTP_11 }.into();
        if let Some(code) = parsed.code {
            builder.status = Some(StatusCode(code));
        }
        for header in parsed.headers.iter() {
            builder.header(header.name, header.value);
        }

        let keep_alive = match builder.get_header(CONNECTION) {
            Some(conn) => !conn.eq_ignore_ascii_case("close"),
            None => !is_http10,
        };

        trace!("received complete headers: {builder:?}");

        let response = builder.build(Vec::new());
        let remaining = self.buf.split_off(header_end);

        HttpCoroutineState::Complete(Ok(Http11ReadHeadersOutput {
            response,
            remaining,
            keep_alive,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_complete_head() {
        let mut coroutine = Http11ReadHeaders::default();
        let reply = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\nbody";

        let out = expect_complete_ok(&mut coroutine, Some(reply));
        assert_eq!(*out.response.status, 200);
        assert_eq!(out.response.version, "HTTP/1.1");
        assert_eq!(out.response.header("content-type"), Some("text/plain"));
        assert_eq!(out.remaining, b"body");
    }

    #[test]
    fn incomplete_head_wants_read() {
        let mut coroutine = Http11ReadHeaders::default();
        expect_wants_read(&mut coroutine, Some(b"HTTP/1.1 200 OK\r\n"));
    }

    #[test]
    fn eof_returns_eof_error() {
        let mut coroutine = Http11ReadHeaders::default();
        let err = expect_complete_err(&mut coroutine, Some(b""));
        assert!(matches!(err, Http11ReadHeadersError::Eof));
    }

    #[test]
    fn http10_keep_alive_defaults_false() {
        let mut coroutine = Http11ReadHeaders::default();
        let reply = b"HTTP/1.0 200 OK\r\n\r\n";
        let out = expect_complete_ok(&mut coroutine, Some(reply));
        assert!(!out.keep_alive);
        assert_eq!(out.response.version, "HTTP/1.0");
    }

    #[test]
    fn http11_keep_alive_defaults_true() {
        let mut coroutine = Http11ReadHeaders::default();
        let reply = b"HTTP/1.1 200 OK\r\n\r\n";
        let out = expect_complete_ok(&mut coroutine, Some(reply));
        assert!(out.keep_alive);
    }

    #[test]
    fn connection_close_overrides_default() {
        let mut coroutine = Http11ReadHeaders::default();
        let reply = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n";
        let out = expect_complete_ok(&mut coroutine, Some(reply));
        assert!(!out.keep_alive);
    }

    // --- utils

    fn expect_wants_read(cor: &mut Http11ReadHeaders, arg: Option<&[u8]>) {
        match cor.resume(arg) {
            HttpCoroutineState::Yielded(HttpYield::WantsRead) => {}
            state => panic!("expected WantsRead, got {state:?}"),
        }
    }

    fn expect_complete_ok(
        cor: &mut Http11ReadHeaders,
        arg: Option<&[u8]>,
    ) -> Http11ReadHeadersOutput {
        match cor.resume(arg) {
            HttpCoroutineState::Complete(Ok(out)) => out,
            state => panic!("expected Complete(Ok), got {state:?}"),
        }
    }

    fn expect_complete_err(
        cor: &mut Http11ReadHeaders,
        arg: Option<&[u8]>,
    ) -> Http11ReadHeadersError {
        match cor.resume(arg) {
            HttpCoroutineState::Complete(Err(err)) => err,
            state => panic!("expected Complete(Err), got {state:?}"),
        }
    }
}
