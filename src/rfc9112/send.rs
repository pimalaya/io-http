//! I/O-free coroutine sending an HTTP/1.1 request and receiving its
//! response ([RFC 9112]).
//!
//! Serialises the request, drives the read/parse cycle through
//! [`Http11HeadersRead`] for the head and selects a body-reading
//! strategy from the response headers:
//!
//! | Strategy     | Trigger                      |
//! |--------------|------------------------------|
//! | Chunked      | `Transfer-Encoding: chunked` |
//! | Fixed-length | `Content-Length: <n>`        |
//! | Read-to-EOF  | Neither header present       |
//!
//! # Example
//!
//! ```rust,no_run
//! use std::{io::{Read, Write}, net::TcpStream};
//!
//! use io_http::{
//!     coroutine::*,
//!     rfc9110::{request::HttpRequest, send::HttpSendYield},
//!     rfc9112::send::Http11Send,
//! };
//! use url::Url;
//!
//! let url = Url::parse("http://example.com/").unwrap();
//! let request = HttpRequest::get(url.clone())
//!     .header("Host", url.host_str().unwrap())
//!     .header("Connection", "close");
//!
//! let mut stream = TcpStream::connect("example.com:80").unwrap();
//! let mut send = Http11Send::new(request);
//! let mut arg: Option<&[u8]> = None;
//! let mut buf = [0u8; 4096];
//!
//! let (response, keep_alive) = loop {
//!     match send.resume(arg.take()) {
//!         HttpCoroutineState::Complete(Ok(out)) => break (out.response, out.keep_alive),
//!         HttpCoroutineState::Complete(Err(err)) => panic!("{err}"),
//!         HttpCoroutineState::Yielded(HttpSendYield::WantsRead) => {
//!             let n = stream.read(&mut buf).unwrap();
//!             arg = Some(&buf[..n]);
//!         }
//!         HttpCoroutineState::Yielded(HttpSendYield::WantsWrite(bytes)) => {
//!             stream.write_all(&bytes).unwrap();
//!         }
//!         HttpCoroutineState::Yielded(HttpSendYield::WantsRedirect { url, .. }) => {
//!             panic!("redirect to {url}");
//!         }
//!     }
//! };
//!
//! println!("{}", *response.status);
//! # let _ = keep_alive;
//! ```
//!
//! [RFC 9112]: https://www.rfc-editor.org/rfc/rfc9112

use core::mem;

use alloc::{borrow::ToOwned, string::String, vec::Vec};

use httparse::Error as HttparseError;
use log::{debug, trace};
use thiserror::Error;
use url::Url;

use crate::{
    coroutine::*,
    http_try,
    rfc1945::version::HTTP_10,
    rfc9110::{
        headers::{HTTP_CONTENT_LENGTH, HTTP_LOCATION, HTTP_TRANSFER_ENCODING},
        request::HttpRequest,
        response::HttpResponse,
        send::{HttpSendOutput, HttpSendYield},
    },
    rfc9112::{
        chunk::{Http11ChunksRead, Http11ChunksReadError},
        read_headers::{Http11HeadersRead, Http11HeadersReadError},
    },
};

/// Failure causes during the HTTP/1.1 send flow.
#[derive(Debug, Error)]
pub enum Http11SendError {
    /// The stream reached EOF before the response was complete.
    #[error("HTTP/1.1 send failed: reached unexpected EOF")]
    Eof,
    /// The response head could not be parsed.
    #[error("HTTP/1.1 send failed: parse response headers: {0}")]
    ParseResponseHeaders(HttparseError),
    /// The content length header value is not a valid integer.
    #[error("HTTP/1.1 send failed: invalid content length `{0}`")]
    InvalidContentLength(String),
    /// The chunked-body decoder failed.
    #[error("HTTP/1.1 send failed: {0}")]
    ReadChunks(#[from] Http11ChunksReadError),
}

impl From<Http11HeadersReadError> for Http11SendError {
    fn from(err: Http11HeadersReadError) -> Self {
        match err {
            Http11HeadersReadError::Eof => Self::Eof,
            Http11HeadersReadError::ParseResponseHeaders(e) => Self::ParseResponseHeaders(e),
        }
    }
}

#[derive(Debug)]
enum State {
    ReadHeaders(Http11HeadersRead),
    BodyChunks(Http11ChunksRead),
    BodyLength(usize),
    BodyEof,
}

/// I/O-free coroutine to send an HTTP/1.1 request and receive its response.
#[derive(Debug)]
pub struct Http11Send {
    request_url: Url,
    state: State,
    wants_write: Option<Vec<u8>>,
    is_conn_closed: bool,
    response: Option<HttpResponse>,
    buf: Vec<u8>,
}

impl Http11Send {
    /// Creates a new coroutine that will send the given request and
    /// receive its response.
    pub fn new(req: HttpRequest) -> Self {
        debug!("prepare request to send");
        trace!("{req:?}");

        let request_url = req.url.clone();
        let bytes = req.to_http_11_vec();

        Self {
            request_url,
            state: State::ReadHeaders(Http11HeadersRead::default()),
            wants_write: Some(bytes),
            is_conn_closed: false,
            response: None,
            buf: Vec::new(),
        }
    }

    fn finish(
        &self,
        response: HttpResponse,
        remaining: Vec<u8>,
    ) -> HttpCoroutineState<HttpSendYield, Result<HttpSendOutput, Http11SendError>> {
        let keep_alive = !self.is_conn_closed;

        if response.status.is_redirection() {
            if let Some(location) = response.header(HTTP_LOCATION) {
                if let Ok(url) = self.request_url.join(location) {
                    let same_scheme = self.request_url.scheme() == url.scheme();
                    let same_host = self.request_url.host() == url.host()
                        && self.request_url.port() == url.port();
                    let same_origin = same_scheme && same_host;

                    return HttpCoroutineState::Yielded(HttpSendYield::WantsRedirect {
                        url,
                        response,
                        keep_alive,
                        same_origin,
                    });
                }
            }
        }

        HttpCoroutineState::Complete(Ok(HttpSendOutput {
            response,
            remaining,
            keep_alive,
        }))
    }
}

impl HttpCoroutine for Http11Send {
    type Yield = HttpSendYield;
    type Return = Result<HttpSendOutput, Http11SendError>;

    fn resume(&mut self, mut arg: Option<&[u8]>) -> HttpCoroutineState<Self::Yield, Self::Return> {
        loop {
            if let Some(bytes) = self.wants_write.take() {
                return HttpCoroutineState::Yielded(HttpSendYield::WantsWrite(bytes));
            }

            match &mut self.state {
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
                        let mut response = out.response;
                        let is_http10 = response.version == HTTP_10;
                        self.is_conn_closed = !out.keep_alive;
                        let status = *response.status;

                        if status == 204 || status == 304 {
                            return self.finish(response, out.remaining);
                        }

                        if !is_http10 {
                            let chunked = response
                                .header(HTTP_TRANSFER_ENCODING)
                                .is_some_and(|enc| enc.eq_ignore_ascii_case("chunked"));
                            if chunked {
                                let mut chunks = Http11ChunksRead::default();
                                match chunks.resume(Some(&out.remaining)) {
                                    HttpCoroutineState::Complete(Ok(chunk_out)) => {
                                        response.body = chunk_out.body;
                                        return self.finish(response, chunk_out.remaining);
                                    }
                                    HttpCoroutineState::Yielded(HttpYield::WantsRead) => {
                                        self.response = Some(response);
                                        debug!("reading chunked body");
                                        self.state = State::BodyChunks(chunks);
                                        return HttpCoroutineState::Yielded(
                                            HttpSendYield::WantsRead,
                                        );
                                    }
                                    HttpCoroutineState::Yielded(HttpYield::WantsWrite(_)) => {
                                        unreachable!("Http11ChunksRead never writes");
                                    }
                                    HttpCoroutineState::Complete(Err(err)) => {
                                        return HttpCoroutineState::Complete(Err(err.into()));
                                    }
                                }
                            }
                        }

                        if let Some(len_str) = response.header(HTTP_CONTENT_LENGTH) {
                            let len_str = len_str.trim();
                            let Ok(len) = len_str.parse::<usize>() else {
                                let err = Http11SendError::InvalidContentLength(len_str.to_owned());
                                return HttpCoroutineState::Complete(Err(err));
                            };
                            self.buf = out.remaining;
                            self.response = Some(response);
                            debug!("reading body of length {len}");
                            self.state = State::BodyLength(len);
                            continue;
                        }

                        self.buf = out.remaining;
                        self.response = Some(response);
                        debug!("reading body until connection closes");
                        self.state = State::BodyEof;
                    }
                },
                State::BodyChunks(chunks) => {
                    let chunk_out = http_try!(chunks, arg.take());
                    let mut response = self.response.take().expect("response missing");
                    response.body = chunk_out.body;
                    return self.finish(response, chunk_out.remaining);
                }
                State::BodyLength(len) => {
                    if let Some(data) = arg.take() {
                        self.buf.extend_from_slice(data);
                    }

                    if *len > self.buf.len() {
                        trace!("received incomplete body {len}/{}", self.buf.len());
                        return HttpCoroutineState::Yielded(HttpSendYield::WantsRead);
                    }

                    let body = self.buf.drain(..*len).collect();
                    let remaining = mem::take(&mut self.buf);
                    let mut response = self.response.take().expect("response missing");
                    response.body = body;
                    return self.finish(response, remaining);
                }
                State::BodyEof => match arg.take() {
                    Some(&[]) => {
                        let buf = mem::take(&mut self.buf);
                        let mut response = self.response.take().expect("response missing");
                        response.body = buf;
                        return self.finish(response, Vec::new());
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

#[cfg(test)]
mod tests {
    use crate::{coroutine::*, rfc9112::send::*};

    #[test]
    fn body_chunks_completes() {
        let req = HttpRequest::get("https://example.com".try_into().unwrap());
        let mut coroutine = Http11Send::new(req);

        let bytes = expect_wants_write(&mut coroutine, None);
        assert_eq!(
            bytes,
            b"GET / HTTP/1.1\r\nhost: example.com\r\ncontent-length: 0\r\n\r\n"
        );

        expect_wants_read(&mut coroutine, None);

        let reply = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        let out = expect_complete_ok(&mut coroutine, Some(reply));
        assert_eq!(out.response.version, "HTTP/1.1");
        assert_eq!(*out.response.status, 200);
        assert_eq!(out.response.body, b"hello world");
        assert!(out.remaining.is_empty());
        assert!(out.keep_alive);
    }

    #[test]
    fn body_length_completes() {
        let req = HttpRequest::get("https://example.com".try_into().unwrap());
        let mut coroutine = Http11Send::new(req);

        expect_wants_write(&mut coroutine, None);
        expect_wants_read(&mut coroutine, None);

        let reply = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello";
        let out = expect_complete_ok(&mut coroutine, Some(reply));
        assert_eq!(*out.response.status, 200);
        assert_eq!(out.response.body, b"hello");
        assert!(out.keep_alive);
    }

    #[test]
    fn body_eof_completes() {
        let req = HttpRequest::get("https://example.com".try_into().unwrap());
        let mut coroutine = Http11Send::new(req);

        expect_wants_write(&mut coroutine, None);
        expect_wants_read(&mut coroutine, None);
        expect_wants_read(&mut coroutine, Some(b"HTTP/1.1 200 OK\r\n\r\nhello "));
        expect_wants_read(&mut coroutine, Some(b"world"));

        let out = expect_complete_ok(&mut coroutine, Some(b""));
        assert_eq!(out.response.body, b"hello world");
    }

    #[test]
    fn invalid_content_length_errors() {
        let req = HttpRequest::get("https://example.com".try_into().unwrap());
        let mut coroutine = Http11Send::new(req);

        expect_wants_write(&mut coroutine, None);
        expect_wants_read(&mut coroutine, None);

        let reply = b"HTTP/1.1 200 OK\r\nContent-Length: notanumber\r\n\r\n";
        let err = expect_complete_err(&mut coroutine, Some(reply));
        let Http11SendError::InvalidContentLength(s) = err else {
            panic!("expected InvalidContentLength, got {err:?}");
        };
        assert_eq!(s, "notanumber");
    }

    #[test]
    fn redirect_yields_wants_redirect() {
        let req = HttpRequest::get("http://example.com/old".try_into().unwrap());
        let mut coroutine = Http11Send::new(req);

        expect_wants_write(&mut coroutine, None);
        expect_wants_read(&mut coroutine, None);

        let reply =
            b"HTTP/1.1 301 Moved Permanently\r\nLocation: /new\r\nContent-Length: 0\r\n\r\n";
        match coroutine.resume(Some(reply)) {
            HttpCoroutineState::Yielded(HttpSendYield::WantsRedirect {
                url,
                same_origin,
                keep_alive,
                ..
            }) => {
                assert_eq!(url.path(), "/new");
                assert!(same_origin);
                assert!(keep_alive);
            }
            state => panic!("expected WantsRedirect, got {state:?}"),
        }
    }

    fn expect_wants_write(cor: &mut Http11Send, arg: Option<&[u8]>) -> Vec<u8> {
        match cor.resume(arg) {
            HttpCoroutineState::Yielded(HttpSendYield::WantsWrite(bytes)) => bytes,
            state => panic!("expected WantsWrite, got {state:?}"),
        }
    }

    fn expect_wants_read(cor: &mut Http11Send, arg: Option<&[u8]>) {
        match cor.resume(arg) {
            HttpCoroutineState::Yielded(HttpSendYield::WantsRead) => {}
            state => panic!("expected WantsRead, got {state:?}"),
        }
    }

    fn expect_complete_ok(cor: &mut Http11Send, arg: Option<&[u8]>) -> HttpSendOutput {
        match cor.resume(arg) {
            HttpCoroutineState::Complete(Ok(out)) => out,
            state => panic!("expected Complete(Ok), got {state:?}"),
        }
    }

    fn expect_complete_err(cor: &mut Http11Send, arg: Option<&[u8]>) -> Http11SendError {
        match cor.resume(arg) {
            HttpCoroutineState::Complete(Err(err)) => err,
            state => panic!("expected Complete(Err), got {state:?}"),
        }
    }
}
