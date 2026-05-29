//! I/O-free coroutine to read and parse an HTTP/1.X response head
//! (RFC 9112 §6).
//!
//! Accumulates bytes from the socket until the `\r\n\r\n` head
//! terminator is seen, parses the status line + headers via
//! `httparse`, and emits an [`HttpResponse`] with an empty body. Any
//! bytes that came past the terminator are returned in `remaining`
//! so the caller can hand them off to a body-decoding coroutine
//! (chunked transfer, content-length, or a streaming consumer such
//! as Server-Sent Events).
//!
//! `keep_alive` is derived from the `Connection` header: present and
//! `close` → `false`; absent → defaults by version (HTTP/1.0 →
//! `false`, HTTP/1.1 → `true`).
//!
//! Composed by [`super::send::Http11Send`] (which dispatches to a
//! body-reading coroutine after the head) and reused by the SSE
//! bootstrap in [`crate::client::HttpClientStd::send_streaming`].

use alloc::vec::Vec;

use httparse::{EMPTY_HEADER, Response, Status};
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

/// Errors that can occur during the coroutine progression.
#[derive(Debug, Error)]
pub enum Http11ReadHeadersError {
    #[error("Reached unexpected EOF before headers were complete")]
    Eof,
    #[error("Parse HTTP response headers error: {0}")]
    ParseResponseHeaders(httparse::Error),
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
            // HTTP/1.0 closes connections by default; HTTP/1.1 keeps
            // them alive.
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
