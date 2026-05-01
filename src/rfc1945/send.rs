//! I/O-free coroutine to send an HTTP request and receive its
//! response (RFC 1945).
//!
//! The coroutine serializes the request, hands the caller the bytes
//! to write, then collects bytes from the caller until the response
//! headers and body are complete. Two body-reading strategies are
//! supported, selected automatically from the response headers:
//!
//! | Strategy     | Trigger               |
//! |--------------|-----------------------|
//! | Fixed-length | `Content-Length: <n>` |
//! | Read-to-EOF  | Header absent         |
//!
//! Unlike HTTP/1.1, chunked transfer encoding is not defined in RFC
//! 1945. Connections always close after each response unless the
//! server sends the non-standard `Connection: keep-alive` header.

use core::mem;

use alloc::{borrow::ToOwned, string::String, vec::Vec};

use httparse::{EMPTY_HEADER, Response, Status};
use log::trace;
use thiserror::Error;
use url::Url;

use crate::{
    rfc1945::version::HTTP_10,
    rfc9110::{
        headers::{CONNECTION, CONTENT_LENGTH, LOCATION},
        request::HttpRequest,
        response::{HttpResponse, ResponseBuilder},
        status::StatusCode,
    },
};

/// Errors that can occur during the coroutine progression.
#[derive(Debug, Error)]
pub enum Http10SendError {
    #[error("Reached unexpected EOF")]
    Eof,
    #[error("Parse HTTP response headers error: {0}")]
    ParseResponseHeaders(httparse::Error),
    #[error("Parse HTTP/1.0 response error: invalid content length `{0}`")]
    InvalidContentLength(String),
}

/// Result returned by [`Http10Send::resume`].
#[derive(Debug)]
pub enum Http10SendResult {
    /// The coroutine has successfully terminated with a non-redirect
    /// response.
    ///
    /// A 3xx response where the `Location` header is absent or
    /// unparseable also arrives here — the caller can inspect
    /// `response.status` if needed.
    Ok {
        /// The response received.
        response: HttpResponse,
        /// Bytes left in the buffer after the body — typically empty
        /// for HTTP/1.0 (connection closes by default).
        remaining: Vec<u8>,
        /// Whether the server indicated the connection can be reused.
        ///
        /// HTTP/1.0 closes connections by default; this is `true`
        /// only when the server sends the non-standard `Connection:
        /// keep-alive` header.
        keep_alive: bool,
    },

    /// The coroutine needs more bytes to be read from the socket.
    WantsRead,

    /// The coroutine wants the given bytes to be written to the
    /// socket.
    WantsWrite(Vec<u8>),

    /// The server responded with a 3xx redirect.
    ///
    /// The caller should create a new [`Http10Send`] targeting `url`.
    /// When `!keep_alive || !same_origin`, a new connection must be
    /// opened before sending the next request.
    WantsRedirect {
        /// Resolved redirect target URL (from the `Location` header).
        url: Url,
        /// The 3xx response received.
        response: HttpResponse,
        /// Whether the server indicated it will keep the connection
        /// open.
        keep_alive: bool,
        /// Whether the redirect stays on the same scheme, host, and
        /// port.
        ///
        /// When `false`, forwarding credentials to the new host
        /// without user consent is inadvisable (RFC 9110 §15.4).
        same_origin: bool,
    },

    /// The coroutine encountered an error.
    Err(Http10SendError),
}

/// Internal state of the [`Http10Send`] coroutine.
#[derive(Debug)]
enum State {
    /// Receive response headers incrementally.
    Headers,
    /// Receive a fixed-length response body.
    BodyLength(usize),
    /// Receive the response body until EOF.
    BodyEof,
}

/// I/O-free coroutine to send an HTTP/1.0 request and receive its response.
///
/// # Example
///
/// ```rust,ignore
/// use std::{io::{Read, Write}, net::TcpStream};
/// use io_http::rfc1945::send::{Http10Send, Http10SendResult};
/// use io_http::rfc9110::request::HttpRequest;
/// use url::Url;
///
/// let url = Url::parse("http://example.com/").unwrap();
/// let request = HttpRequest::get(url.clone())
///     .header("Host", url.host_str().unwrap());
///
/// let mut stream = TcpStream::connect("example.com:80").unwrap();
/// let mut send = Http10Send::new(request);
/// let mut arg: Option<&[u8]> = None;
/// let mut buf = [0u8; 4096];
///
/// let response = loop {
///     match send.resume(arg.take()) {
///         Http10SendResult::Ok { response, .. } => break response,
///         Http10SendResult::Err(err) => panic!("{err}"),
///         Http10SendResult::WantsWrite(bytes) => {
///             stream.write_all(&bytes).unwrap();
///         }
///         Http10SendResult::WantsRead => {
///             let n = stream.read(&mut buf).unwrap();
///             arg = Some(&buf[..n]);
///         }
///         Http10SendResult::WantsRedirect { .. } => unimplemented!(),
///     }
/// };
///
/// println!("{}", *response.status);
/// ```
#[derive(Debug)]
pub struct Http10Send {
    request_url: Url,
    state: State,
    wants_read: bool,
    wants_write: Option<Vec<u8>>,
    keep_alive: bool,
    buf: Vec<u8>,
    response: ResponseBuilder,
}

impl Http10Send {
    /// Creates a new coroutine that will send the given request and
    /// receive its response.
    pub fn new(req: HttpRequest) -> Self {
        trace!("prepares HTTP/1.0 request to be sent: {req:?}");

        let request_url = req.url.clone();
        let bytes = req.to_http_10_vec();

        Self {
            request_url,
            state: State::Headers,
            wants_read: false,
            wants_write: Some(bytes),
            keep_alive: false,
            buf: Vec::new(),
            response: ResponseBuilder::default(),
        }
    }

    /// Advances the coroutine.
    ///
    /// Pass [`None`] when there is no data to provide (initial call,
    /// after a write). Pass `Some(data)` with bytes read from the
    /// socket after a [`Http10SendResult::WantsRead`]. Pass `Some(&[])`
    /// to signal EOF.
    pub fn resume(&mut self, mut arg: Option<&[u8]>) -> Http10SendResult {
        loop {
            if let Some(bytes) = self.wants_write.take() {
                return Http10SendResult::WantsWrite(bytes);
            }

            if mem::take(&mut self.wants_read) {
                return Http10SendResult::WantsRead;
            }

            match &mut self.state {
                State::Headers => {
                    trace!("state: headers");

                    match arg.take() {
                        Some(&[]) => break Http10SendResult::Err(Http10SendError::Eof),
                        Some(data) => self.buf.extend_from_slice(data),
                        None => {}
                    }

                    let mut headers = [EMPTY_HEADER; 64];
                    let mut headers = Response::new(&mut headers);

                    let n = match headers.parse(&self.buf) {
                        Ok(Status::Complete(n)) => n,
                        Ok(Status::Partial) => {
                            trace!("received incomplete headers");
                            self.wants_read = true;
                            continue;
                        }
                        Err(err) => {
                            let err = Http10SendError::ParseResponseHeaders(err);
                            break Http10SendResult::Err(err);
                        }
                    };

                    let mut no_content = false;
                    self.response.version = HTTP_10.into();

                    if let Some(code) = headers.code {
                        no_content = code == 204 || code == 304;
                        self.response.status = Some(StatusCode(code));
                    }

                    for header in headers.headers {
                        self.response.header(header.name, header.value);
                    }

                    // HTTP/1.0 closes connections by default; only
                    // honor a non-standard `Connection: keep-alive`.
                    self.keep_alive = self
                        .response
                        .get_header(CONNECTION)
                        .is_some_and(|conn| conn.eq_ignore_ascii_case("keep-alive"));

                    trace!("received complete headers: {:?}", self.response);

                    if no_content {
                        let response = mem::take(&mut self.response).build(Vec::new());
                        break self.finish(response, Vec::new());
                    }

                    // remove headers from buffer, leaving pre-read body in place
                    self.buf.drain(..n);

                    if let Some(len) = self.response.get_header(CONTENT_LENGTH) {
                        let len = len.trim();

                        let Ok(len) = usize::from_str_radix(len, 10) else {
                            let err = Http10SendError::InvalidContentLength(len.to_owned());
                            return Http10SendResult::Err(err);
                        };

                        self.state = State::BodyLength(len);
                        continue;
                    }

                    self.state = State::BodyEof;
                }
                State::BodyLength(len) => {
                    trace!("state: body length");

                    if let Some(data) = arg.take() {
                        self.buf.extend_from_slice(data);
                    }

                    if *len > self.buf.len() {
                        trace!("received incomplete body {len}/{}", self.buf.len());
                        self.wants_read = true;
                    } else {
                        let body = self.buf.drain(..*len).collect();
                        let remaining = mem::take(&mut self.buf);
                        let response = mem::take(&mut self.response).build(body);
                        return self.finish(response, remaining);
                    }
                }
                State::BodyEof => {
                    trace!("state: body eof");

                    match arg.take() {
                        Some(&[]) => {
                            let buf = mem::take(&mut self.buf);
                            let response = mem::take(&mut self.response).build(buf);
                            break self.finish(response, Vec::new());
                        }
                        Some(data) => {
                            self.buf.extend_from_slice(data);
                            self.wants_read = true;
                        }
                        None => {
                            self.wants_read = true;
                        }
                    }
                }
            }
        }
    }

    /// Builds the terminal result for the given response, emitting
    /// [`Http10SendResult::WantsRedirect`] when the status is 3xx and
    /// the `Location` header resolves to a valid URL, otherwise
    /// [`Http10SendResult::Ok`].
    fn finish(&self, response: HttpResponse, remaining: Vec<u8>) -> Http10SendResult {
        let keep_alive = self.keep_alive;

        if response.status.is_redirection() {
            if let Some(location) = response.header(LOCATION) {
                if let Ok(url) = self.request_url.join(location) {
                    let same_scheme = self.request_url.scheme() == url.scheme();
                    let same_host = self.request_url.host() == url.host()
                        && self.request_url.port() == url.port();
                    let same_origin = same_scheme && same_host;

                    return Http10SendResult::WantsRedirect {
                        url,
                        response,
                        keep_alive,
                        same_origin,
                    };
                }
            }
        }

        Http10SendResult::Ok {
            response,
            remaining,
            keep_alive,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_length() {
        let req = HttpRequest::get("http://example.com".try_into().unwrap());
        let mut coroutine = Http10Send::new(req);
        let mut buf: Option<&[u8]> = None;

        loop {
            match coroutine.resume(buf) {
                Http10SendResult::Ok {
                    response,
                    remaining,
                    keep_alive,
                } => {
                    assert_eq!("HTTP/1.0", response.version);
                    assert_eq!(200, *response.status);
                    assert_eq!(b"hello", &*response.body);
                    assert_eq!(0, remaining.len());
                    assert_eq!(false, keep_alive);
                    break;
                }
                Http10SendResult::WantsWrite(bytes) => {
                    assert_eq!(bytes, b"GET / HTTP/1.0\r\ncontent-length: 0\r\n\r\n");
                    buf = None;
                }
                Http10SendResult::WantsRead => {
                    buf = Some(b"HTTP/1.0 200 OK\r\nContent-Length: 5\r\n\r\nhello");
                }
                Http10SendResult::WantsRedirect { .. } => unreachable!("wants redirect"),
                Http10SendResult::Err(err) => unreachable!("{err}"),
            }
        }
    }

    #[test]
    fn body_eof() {
        let req = HttpRequest::get("http://example.com".try_into().unwrap());
        let mut coroutine = Http10Send::new(req);
        let mut buf: Option<&[u8]> = None;
        let mut count = 0;

        loop {
            match coroutine.resume(buf) {
                Http10SendResult::Ok {
                    response,
                    remaining,
                    keep_alive,
                } => {
                    assert_eq!("HTTP/1.0", response.version);
                    assert_eq!(200, *response.status);
                    assert_eq!(b"hello world", &*response.body);
                    assert_eq!(0, remaining.len());
                    assert_eq!(false, keep_alive);
                    break;
                }
                Http10SendResult::WantsWrite(bytes) => {
                    assert_eq!(bytes, b"GET / HTTP/1.0\r\ncontent-length: 0\r\n\r\n");
                    buf = None;
                }
                Http10SendResult::WantsRead if count == 0 => {
                    count = 1;
                    buf = Some(b"HTTP/1.0 200 OK\r\n\r\nhello ");
                }
                Http10SendResult::WantsRead if count == 1 => {
                    count = 2;
                    buf = Some(b"world");
                }
                Http10SendResult::WantsRead => {
                    buf = Some(b"");
                }
                Http10SendResult::WantsRedirect { .. } => unreachable!("wants redirect"),
                Http10SendResult::Err(err) => unreachable!("{err}"),
            }
        }
    }

    #[test]
    fn keep_alive_when_server_says_so() {
        let req = HttpRequest::get("http://example.com".try_into().unwrap());
        let mut coroutine = Http10Send::new(req);
        let mut buf: Option<&[u8]> = None;

        loop {
            match coroutine.resume(buf) {
                Http10SendResult::Ok { keep_alive, .. } => {
                    assert!(keep_alive);
                    break;
                }
                Http10SendResult::WantsWrite(_) => buf = None,
                Http10SendResult::WantsRead => {
                    buf = Some(
                        b"HTTP/1.0 200 OK\r\nConnection: keep-alive\r\nContent-Length: 0\r\n\r\n",
                    );
                }
                Http10SendResult::WantsRedirect { .. } => unreachable!("wants redirect"),
                Http10SendResult::Err(err) => unreachable!("{err}"),
            }
        }
    }
}
