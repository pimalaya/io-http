//! I/O-free coroutine reading and parsing an HTTP/1.X response head
//! ([RFC 9112 §6]). Composed by `Http10Send`, `Http11Send`, and the SSE
//! bootstrap.
//!
//! [RFC 9112 §6]: https://www.rfc-editor.org/rfc/rfc9112#section-6

use alloc::{vec, vec::Vec};

use httparse::{EMPTY_HEADER, Error as HttparseError, Response, Status};
use log::{debug, trace};
use thiserror::Error;

use crate::{
    coroutine::*,
    rfc1945::version::HTTP_10,
    rfc9110::{
        headers::HTTP_CONNECTION,
        response::{HttpResponse, HttpResponseBuilder},
        status::HttpStatusCode,
    },
    rfc9112::version::HTTP_11,
};

/// Failure causes during the HTTP/1.X read-headers flow.
#[derive(Debug, Error)]
pub enum Http11HeadersReadError {
    /// The stream reached EOF before the head was complete.
    #[error("HTTP/1.X read headers failed: reached unexpected EOF before headers were complete")]
    Eof,
    /// The response head could not be parsed.
    #[error("HTTP/1.X read headers failed: parse response headers: {0}")]
    ParseResponseHeaders(HttparseError),
    /// The response head exceeds [`Http11HeadersRead::max_len`].
    #[error("HTTP/1.X read headers failed: response head exceeds {0} bytes")]
    TooLarge(usize),
    /// The server sent over [`HTTP11_MAX_INTERIM_RESPONSES`] interim
    /// responses.
    #[error("HTTP/1.X read headers failed: more than {0} interim responses")]
    TooManyInterimResponses(usize),
}

/// Terminal output of [`Http11HeadersRead`]; the response body is
/// empty at this stage.
#[derive(Debug)]
pub struct Http11HeadersReadOutput {
    /// The parsed response, head only.
    pub response: HttpResponse,
    /// Bytes already buffered past the end of the head.
    pub remaining: Vec<u8>,
    /// Whether the server signalled the connection can be reused.
    pub keep_alive: bool,
}

/// Default value of [`Http11HeadersRead::max_len`]: 64 KiB.
pub const HTTP_DEFAULT_MAX_HEADERS_LEN: usize = 64 * 1024;

/// Maximum number of interim 1xx responses skipped before the final
/// one: 8.
pub const HTTP11_MAX_INTERIM_RESPONSES: usize = 8;

/// I/O-free coroutine to read and parse an HTTP/1.X response head.
/// Interim 1xx responses are skipped, except 101.
#[derive(Debug)]
pub struct Http11HeadersRead {
    /// Maximum size in bytes of the response head, status line and
    /// terminating empty line included. A peer exceeding it fails the
    /// read with [`Http11HeadersReadError::TooLarge`] instead of growing
    /// the buffer without bound. It is the only limit on the number of
    /// headers.
    pub max_len: usize,
    buf: Vec<u8>,
    /// How many headers the next parse has room for.
    headers_cap: usize,
    /// How many interim responses were skipped.
    interim: usize,
}

impl Default for Http11HeadersRead {
    fn default() -> Self {
        Self {
            max_len: HTTP_DEFAULT_MAX_HEADERS_LEN,
            buf: Vec::new(),
            headers_cap: 64,
            interim: 0,
        }
    }
}

impl HttpCoroutine for Http11HeadersRead {
    type Yield = HttpYield;
    type Return = Result<Http11HeadersReadOutput, Http11HeadersReadError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> HttpCoroutineState<Self::Yield, Self::Return> {
        match arg {
            Some(&[]) => {
                return HttpCoroutineState::Complete(Err(Http11HeadersReadError::Eof));
            }
            Some(data) => self.buf.extend_from_slice(data),
            None => {}
        }

        loop {
            let window = &self.buf[..self.buf.len().min(self.max_len)];
            let mut headers = vec![EMPTY_HEADER; self.headers_cap];
            let mut parsed = Response::new(&mut headers);

            let header_end = match parsed.parse(window) {
                Ok(Status::Complete(n)) => n,
                Ok(Status::Partial) if self.buf.len() > self.max_len => {
                    let err = Http11HeadersReadError::TooLarge(self.max_len);
                    return HttpCoroutineState::Complete(Err(err));
                }
                Ok(Status::Partial) => {
                    trace!("received incomplete headers");
                    return HttpCoroutineState::Yielded(HttpYield::WantsRead);
                }
                Err(HttparseError::TooManyHeaders) => {
                    self.headers_cap *= 2;
                    continue;
                }
                Err(err) => {
                    return HttpCoroutineState::Complete(Err(
                        Http11HeadersReadError::ParseResponseHeaders(err),
                    ));
                }
            };

            let status = parsed.code.map(HttpStatusCode);

            // NOTE: 101 switches protocols, which makes it a final response
            if status.is_some_and(|status| status.is_informational() && *status != 101) {
                if self.interim == HTTP11_MAX_INTERIM_RESPONSES {
                    let limit = HTTP11_MAX_INTERIM_RESPONSES;
                    let err = Http11HeadersReadError::TooManyInterimResponses(limit);
                    return HttpCoroutineState::Complete(Err(err));
                }

                debug!("skip interim response");
                self.interim += 1;
                self.buf.drain(..header_end);
                continue;
            }

            let mut builder = HttpResponseBuilder::default();
            let is_http10 = matches!(parsed.version, Some(0));
            builder.version = if is_http10 { HTTP_10 } else { HTTP_11 }.into();
            builder.status = status;
            for header in parsed.headers.iter() {
                builder.header(header.name, header.value);
            }

            debug!("received complete headers");
            trace!("{builder:?}");

            let response = builder.build(Vec::new());
            let remaining = self.buf.split_off(header_end);

            let has_option = |option: &str| {
                response
                    .header_list(HTTP_CONNECTION)
                    .any(|o| o.eq_ignore_ascii_case(option))
            };

            let keep_alive = !has_option("close") && (!is_http10 || has_option("keep-alive"));

            return HttpCoroutineState::Complete(Ok(Http11HeadersReadOutput {
                response,
                remaining,
                keep_alive,
            }));
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rfc9112::read_headers::*;

    #[test]
    fn parses_complete_head() {
        let mut coroutine = Http11HeadersRead::default();
        let reply = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\nbody";

        let out = expect_complete_ok(&mut coroutine, Some(reply));
        assert_eq!(*out.response.status, 200);
        assert_eq!(out.response.version, "HTTP/1.1");
        assert_eq!(out.response.header("content-type"), Some("text/plain"));
        assert_eq!(out.remaining, b"body");
    }

    #[test]
    fn incomplete_head_wants_read() {
        let mut coroutine = Http11HeadersRead::default();
        expect_wants_read(&mut coroutine, Some(b"HTTP/1.1 200 OK\r\n"));
    }

    #[test]
    fn eof_returns_eof_error() {
        let mut coroutine = Http11HeadersRead::default();
        let err = expect_complete_err(&mut coroutine, Some(b""));
        assert!(matches!(err, Http11HeadersReadError::Eof));
    }

    #[test]
    fn http10_keep_alive_defaults_false() {
        let mut coroutine = Http11HeadersRead::default();
        let reply = b"HTTP/1.0 200 OK\r\n\r\n";
        let out = expect_complete_ok(&mut coroutine, Some(reply));
        assert!(!out.keep_alive);
        assert_eq!(out.response.version, "HTTP/1.0");
    }

    #[test]
    fn http11_keep_alive_defaults_true() {
        let mut coroutine = Http11HeadersRead::default();
        let reply = b"HTTP/1.1 200 OK\r\n\r\n";
        let out = expect_complete_ok(&mut coroutine, Some(reply));
        assert!(out.keep_alive);
    }

    #[test]
    fn connection_close_overrides_default() {
        let mut coroutine = Http11HeadersRead::default();
        let reply = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n";
        let out = expect_complete_ok(&mut coroutine, Some(reply));
        assert!(!out.keep_alive);
    }

    #[test]
    fn partial_head_over_limit_errors() {
        let mut coroutine = Http11HeadersRead {
            max_len: 32,
            ..Http11HeadersRead::default()
        };
        expect_wants_read(&mut coroutine, Some(b"HTTP/1.1 200 OK\r\n"));
        let err = expect_complete_err(&mut coroutine, Some(b"X-Filler: aaaaaaaaaaaaaaaa\r\n"));
        assert!(matches!(err, Http11HeadersReadError::TooLarge(32)));
    }

    #[test]
    fn complete_head_over_limit_errors() {
        let mut coroutine = Http11HeadersRead {
            max_len: 16,
            ..Http11HeadersRead::default()
        };
        let err = expect_complete_err(&mut coroutine, Some(b"HTTP/1.1 200 OK\r\n\r\n"));
        assert!(matches!(err, Http11HeadersReadError::TooLarge(16)));
    }

    fn expect_wants_read(cor: &mut Http11HeadersRead, arg: Option<&[u8]>) {
        match cor.resume(arg) {
            HttpCoroutineState::Yielded(HttpYield::WantsRead) => {}
            state => panic!("expected WantsRead, got {state:?}"),
        }
    }

    fn expect_complete_ok(
        cor: &mut Http11HeadersRead,
        arg: Option<&[u8]>,
    ) -> Http11HeadersReadOutput {
        match cor.resume(arg) {
            HttpCoroutineState::Complete(Ok(out)) => out,
            state => panic!("expected Complete(Ok), got {state:?}"),
        }
    }

    fn expect_complete_err(
        cor: &mut Http11HeadersRead,
        arg: Option<&[u8]>,
    ) -> Http11HeadersReadError {
        match cor.resume(arg) {
            HttpCoroutineState::Complete(Err(err)) => err,
            state => panic!("expected Complete(Err), got {state:?}"),
        }
    }
}
