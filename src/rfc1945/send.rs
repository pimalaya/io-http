//! I/O-free coroutine sending an HTTP/1.0 request and receiving its response
//! ([RFC 1945]).
//!
//! Same shape as [`crate::rfc9112::send::Http11Send`] but with the HTTP/1.0
//! wire format: no chunked transfer coding, connections close by default unless
//! the server returns `Connection: keep-alive`.
//!
//! | Strategy     | Trigger                                 |
//! |--------------|-----------------------------------------|
//! | No body      | HEAD request, or status 1xx, 204 or 304 |
//! | Fixed-length | `Content-Length: <n>`                   |
//! | Read-to-EOF  | Header absent                           |
//!
//! # Example
//!
//! ```rust,no_run
//! use std::{io::{Read, Write}, net::TcpStream};
//!
//! use io_http::{
//!     coroutine::*,
//!     rfc1945::send::Http10Send,
//!     rfc9110::{request::HttpRequest, send::HttpSendYield},
//! };
//! use url::Url;
//!
//! let url = Url::parse("http://example.com/").unwrap();
//! let request = HttpRequest::get(url.clone())
//!     .header("Host", url.host_str().unwrap());
//!
//! let mut stream = TcpStream::connect("example.com:80").unwrap();
//! let mut send = Http10Send::new(request);
//! let mut arg: Option<&[u8]> = None;
//! let mut buf = [0u8; 4096];
//!
//! let response = loop {
//!     match send.resume(arg.take()) {
//!         HttpCoroutineState::Complete(Ok(out)) => break out.response,
//!         HttpCoroutineState::Complete(Err(err)) => panic!("{err}"),
//!         HttpCoroutineState::Yielded(HttpSendYield::WantsWrite(bytes)) => {
//!             stream.write_all(&bytes).unwrap();
//!         }
//!         HttpCoroutineState::Yielded(HttpSendYield::WantsRead) => {
//!             let n = stream.read(&mut buf).unwrap();
//!             arg = Some(&buf[..n]);
//!         }
//!         HttpCoroutineState::Yielded(HttpSendYield::WantsRedirect { .. }) => unimplemented!(),
//!     }
//! };
//!
//! println!("{}", *response.status);
//! ```
//!
//! [RFC 1945]: https://www.rfc-editor.org/rfc/rfc1945

use core::mem;

use alloc::{string::String, vec::Vec};

use httparse::Error as HttparseError;
use log::{debug, trace};
use thiserror::Error;
use url::Url;

use crate::{
    coroutine::*,
    rfc9110::{
        headers::HTTP_LOCATION,
        request::{HttpRequest, HttpRequestError},
        response::HttpResponse,
        send::{HTTP_DEFAULT_MAX_BODY_LEN, HttpSendOutput, HttpSendYield},
    },
    rfc9112::read_headers::{
        HTTP_DEFAULT_MAX_HEADERS_LEN, Http11HeadersRead, Http11HeadersReadError,
    },
};

/// Failure causes during the HTTP/1.0 send flow.
#[derive(Debug, Error)]
pub enum Http10SendError {
    /// The stream reached EOF before the response was complete.
    #[error("HTTP/1.0 send failed: reached unexpected EOF")]
    Eof,
    /// The response head could not be parsed.
    #[error("HTTP/1.0 send failed: parse response headers: {0}")]
    ParseResponseHeaders(HttparseError),
    /// A content length value is not a valid integer, or two differ.
    #[error("HTTP/1.0 send failed: invalid content length `{0}`")]
    InvalidContentLength(String),
    /// The response head exceeds [`Http10Send::max_headers_len`].
    #[error("HTTP/1.0 send failed: response head exceeds {0} bytes")]
    HeadersTooLarge(usize),
    /// The response body exceeds [`Http10Send::max_body_len`].
    #[error("HTTP/1.0 send failed: response body exceeds {0} bytes")]
    BodyTooLarge(usize),
    /// The server sent too many interim responses.
    #[error("HTTP/1.0 send failed: more than {0} interim responses")]
    TooManyInterimResponses(usize),
    /// The request cannot be serialised, nothing was written.
    #[error("HTTP/1.0 send failed: {0}")]
    InvalidRequest(#[from] HttpRequestError),
}

impl From<Http11HeadersReadError> for Http10SendError {
    fn from(err: Http11HeadersReadError) -> Self {
        match err {
            Http11HeadersReadError::Eof => Self::Eof,
            Http11HeadersReadError::ParseResponseHeaders(e) => Self::ParseResponseHeaders(e),
            Http11HeadersReadError::TooLarge(limit) => Self::HeadersTooLarge(limit),
            Http11HeadersReadError::TooManyInterimResponses(limit) => {
                Self::TooManyInterimResponses(limit)
            }
        }
    }
}

/// I/O-free coroutine to send an HTTP/1.0 request and receive its response.
#[derive(Debug)]
pub struct Http10Send {
    /// Maximum size in bytes of the response head. Defaults to
    /// [`HTTP_DEFAULT_MAX_HEADERS_LEN`].
    pub max_headers_len: usize,
    /// Maximum size in bytes of the response body, whatever its
    /// framing (fixed-length or read-to-EOF). Defaults to
    /// [`HTTP_DEFAULT_MAX_BODY_LEN`].
    pub max_body_len: usize,
    request_url: Url,
    is_head: bool,
    state: State,
    wants_write: Option<Vec<u8>>,
    keep_alive: bool,
    response: Option<HttpResponse>,
    buf: Vec<u8>,
}

impl Http10Send {
    /// Creates a new coroutine that will send the given request and
    /// receive its response. A request that cannot be serialised fails
    /// the first resume with [`Http10SendError::InvalidRequest`].
    pub fn new(req: HttpRequest) -> Self {
        debug!("prepare request to send");
        trace!("{req:?}");

        let request_url = req.url.clone();
        let is_head = req.method == "HEAD";

        let (state, wants_write) = match req.to_http_10_vec() {
            Ok(bytes) => (
                State::ReadHeaders(Http11HeadersRead::default()),
                Some(bytes),
            ),
            Err(err) => (State::InvalidRequest(err), None),
        };

        Self {
            max_headers_len: HTTP_DEFAULT_MAX_HEADERS_LEN,
            max_body_len: HTTP_DEFAULT_MAX_BODY_LEN,
            request_url,
            is_head,
            state,
            wants_write,
            keep_alive: false,
            response: None,
            buf: Vec::new(),
        }
    }

    fn finish(
        &self,
        response: HttpResponse,
        remaining: Vec<u8>,
    ) -> HttpCoroutineState<HttpSendYield, Result<HttpSendOutput, Http10SendError>> {
        let keep_alive = self.keep_alive;

        if response.status.is_redirection()
            && let Some(location) = response.header(HTTP_LOCATION)
            && let Ok(url) = self.request_url.join(location)
        {
            let same_scheme = self.request_url.scheme() == url.scheme();
            let same_host =
                self.request_url.host() == url.host() && self.request_url.port() == url.port();
            let same_origin = same_scheme && same_host;

            return HttpCoroutineState::Yielded(HttpSendYield::WantsRedirect {
                url,
                response,
                keep_alive,
                same_origin,
            });
        }

        HttpCoroutineState::Complete(Ok(HttpSendOutput {
            response,
            remaining,
            keep_alive,
        }))
    }
}

impl HttpCoroutine for Http10Send {
    type Yield = HttpSendYield;
    type Return = Result<HttpSendOutput, Http10SendError>;

    fn resume(&mut self, mut arg: Option<&[u8]>) -> HttpCoroutineState<Self::Yield, Self::Return> {
        loop {
            if let Some(bytes) = self.wants_write.take() {
                return HttpCoroutineState::Yielded(HttpSendYield::WantsWrite(bytes));
            }

            if let State::ReadHeaders(rh) = &mut self.state {
                rh.max_len = self.max_headers_len;
            }

            match &mut self.state {
                State::InvalidRequest(err) => {
                    return HttpCoroutineState::Complete(Err(err.clone().into()));
                }
                State::ReadHeaders(rh) => match rh.resume(arg.take()) {
                    HttpCoroutineState::Yielded(HttpYield::WantsRead) => {
                        return HttpCoroutineState::Yielded(HttpSendYield::WantsRead);
                    }
                    HttpCoroutineState::Yielded(HttpYield::WantsWrite(_)) => {
                        unreachable!("Http11HeadersRead never writes");
                    }
                    HttpCoroutineState::Complete(Err(err)) => {
                        return HttpCoroutineState::Complete(Err(err.into()));
                    }
                    HttpCoroutineState::Complete(Ok(out)) => {
                        let response = out.response;
                        self.keep_alive = out.keep_alive;
                        let status = *response.status;

                        let no_body = response.status.is_informational()
                            || status == 204
                            || status == 304
                            || self.is_head;

                        if no_body {
                            return self.finish(response, out.remaining);
                        }

                        let len = match response.content_length() {
                            Ok(len) => len,
                            Err(len) => {
                                let err = Http10SendError::InvalidContentLength(len);
                                return HttpCoroutineState::Complete(Err(err));
                            }
                        };

                        if let Some(len) = len {
                            if len > self.max_body_len {
                                let err = Http10SendError::BodyTooLarge(self.max_body_len);
                                return HttpCoroutineState::Complete(Err(err));
                            }

                            self.buf = out.remaining;
                            self.response = Some(response);
                            debug!("reading body of length {len}");

                            // NOTE: one block for the body, growth as
                            // usual when the allocator refuses it
                            let missing = len.saturating_sub(self.buf.len());
                            let _ = self.buf.try_reserve_exact(missing);

                            self.state = State::BodyLength(len);
                            continue;
                        }

                        self.buf = out.remaining;
                        self.response = Some(response);
                        debug!("reading body until connection closes");
                        self.keep_alive = false;
                        self.state = State::BodyEof;
                    }
                },
                State::BodyLength(len) => {
                    match arg.take() {
                        Some(&[]) if *len > self.buf.len() => {
                            return HttpCoroutineState::Complete(Err(Http10SendError::Eof));
                        }
                        Some(data) => self.buf.extend_from_slice(data),
                        None => {}
                    }

                    if *len > self.buf.len() {
                        trace!("received incomplete body {len}/{}", self.buf.len());
                        return HttpCoroutineState::Yielded(HttpSendYield::WantsRead);
                    }

                    let remaining = self.buf.split_off(*len);
                    let body = mem::take(&mut self.buf);
                    let mut response = self.response.take().expect("response missing");
                    response.body = body;
                    return self.finish(response, remaining);
                }
                State::BodyEof => match arg.take() {
                    _ if self.buf.len() > self.max_body_len => {
                        let err = Http10SendError::BodyTooLarge(self.max_body_len);
                        return HttpCoroutineState::Complete(Err(err));
                    }
                    Some(&[]) => {
                        let buf = mem::take(&mut self.buf);
                        let mut response = self.response.take().expect("response missing");
                        response.body = buf;
                        return self.finish(response, Vec::new());
                    }
                    Some(data) if data.len() > self.max_body_len.saturating_sub(self.buf.len()) => {
                        let err = Http10SendError::BodyTooLarge(self.max_body_len);
                        return HttpCoroutineState::Complete(Err(err));
                    }
                    Some(data) => {
                        self.buf.extend_from_slice(data);
                        return HttpCoroutineState::Yielded(HttpSendYield::WantsRead);
                    }
                    None => {
                        return HttpCoroutineState::Yielded(HttpSendYield::WantsRead);
                    }
                },
            }
        }
    }
}

#[derive(Debug)]
enum State {
    InvalidRequest(HttpRequestError),
    ReadHeaders(Http11HeadersRead),
    BodyLength(usize),
    BodyEof,
}

#[cfg(test)]
mod tests {
    use crate::rfc1945::send::*;

    #[test]
    fn body_length_completes() {
        let req = HttpRequest::get("http://example.com".try_into().unwrap());
        let mut coroutine = Http10Send::new(req);

        let bytes = expect_wants_write(&mut coroutine, None);
        assert_eq!(bytes, b"GET / HTTP/1.0\r\ncontent-length: 0\r\n\r\n");

        expect_wants_read(&mut coroutine, None);

        let reply = b"HTTP/1.0 200 OK\r\nContent-Length: 5\r\n\r\nhello";
        let out = expect_complete_ok(&mut coroutine, Some(reply));
        assert_eq!(out.response.version, "HTTP/1.0");
        assert_eq!(*out.response.status, 200);
        assert_eq!(out.response.body, b"hello");
        assert!(!out.keep_alive);
    }

    #[test]
    fn body_eof_completes() {
        let req = HttpRequest::get("http://example.com".try_into().unwrap());
        let mut coroutine = Http10Send::new(req);

        expect_wants_write(&mut coroutine, None);
        expect_wants_read(&mut coroutine, None);
        expect_wants_read(&mut coroutine, Some(b"HTTP/1.0 200 OK\r\n\r\nhello "));
        expect_wants_read(&mut coroutine, Some(b"world"));

        let out = expect_complete_ok(&mut coroutine, Some(b""));
        assert_eq!(out.response.body, b"hello world");
        assert!(!out.keep_alive);
    }

    #[test]
    fn keep_alive_when_server_says_so() {
        let req = HttpRequest::get("http://example.com".try_into().unwrap());
        let mut coroutine = Http10Send::new(req);

        expect_wants_write(&mut coroutine, None);
        expect_wants_read(&mut coroutine, None);

        let reply = b"HTTP/1.0 200 OK\r\nConnection: keep-alive\r\nContent-Length: 0\r\n\r\n";
        let out = expect_complete_ok(&mut coroutine, Some(reply));
        assert!(out.keep_alive);
    }

    #[test]
    fn invalid_content_length_errors() {
        let req = HttpRequest::get("http://example.com".try_into().unwrap());
        let mut coroutine = Http10Send::new(req);

        expect_wants_write(&mut coroutine, None);
        expect_wants_read(&mut coroutine, None);

        let reply = b"HTTP/1.0 200 OK\r\nContent-Length: notanumber\r\n\r\n";
        let err = expect_complete_err(&mut coroutine, Some(reply));
        let Http10SendError::InvalidContentLength(s) = err else {
            panic!("expected InvalidContentLength, got {err:?}");
        };
        assert_eq!(s, "notanumber");
    }

    fn expect_wants_write(cor: &mut Http10Send, arg: Option<&[u8]>) -> Vec<u8> {
        match cor.resume(arg) {
            HttpCoroutineState::Yielded(HttpSendYield::WantsWrite(bytes)) => bytes,
            state => panic!("expected WantsWrite, got {state:?}"),
        }
    }

    fn expect_wants_read(cor: &mut Http10Send, arg: Option<&[u8]>) {
        match cor.resume(arg) {
            HttpCoroutineState::Yielded(HttpSendYield::WantsRead) => {}
            state => panic!("expected WantsRead, got {state:?}"),
        }
    }

    fn expect_complete_ok(cor: &mut Http10Send, arg: Option<&[u8]>) -> HttpSendOutput {
        match cor.resume(arg) {
            HttpCoroutineState::Complete(Ok(out)) => out,
            state => panic!("expected Complete(Ok), got {state:?}"),
        }
    }

    fn expect_complete_err(cor: &mut Http10Send, arg: Option<&[u8]>) -> Http10SendError {
        match cor.resume(arg) {
            HttpCoroutineState::Complete(Err(err)) => err,
            state => panic!("expected Complete(Err), got {state:?}"),
        }
    }
}
