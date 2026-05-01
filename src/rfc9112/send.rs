//! I/O-free coroutine to send an HTTP request and receive its
//! response (RFC 9112).
//!
//! The coroutine serializes the request, writes it to the socket,
//! then reads and parses the response headers and body. Three
//! body-reading strategies are supported, selected automatically from
//! the response headers:
//!
//! | Strategy     | Trigger                      |
//! |--------------|------------------------------|
//! | Chunked      | `Transfer-Encoding: chunked` |
//! | Fixed-length | `Content-Length: <n>`        |
//! | Read-to-EOF  | Neither header present       |

use core::mem;

use alloc::{borrow::ToOwned, string::String, vec::Vec};

use httparse::{EMPTY_HEADER, Response, Status};
use log::trace;
use thiserror::Error;
use url::Url;

use crate::{
    rfc1945::version::HTTP_10,
    rfc9110::{
        headers::{CONNECTION, CONTENT_LENGTH, LOCATION, TRANSFER_ENCODING},
        request::HttpRequest,
        response::{HttpResponse, ResponseBuilder},
        status::StatusCode,
    },
    rfc9112::{chunk::*, version::HTTP_11},
};

/// Errors that can occur during the coroutine progression.
#[derive(Debug, Error)]
pub enum Http11SendError {
    #[error("Reached unexpected EOF")]
    Eof,
    #[error("Parse HTTP response headers error: {0}")]
    ParseResponseHeaders(httparse::Error),
    #[error("Parse HTTP/1.1 response error: invalid content length `{0}`")]
    InvalidContentLength(String),
    #[error(transparent)]
    ReadChunks(#[from] Http11ReadChunksError),
}

/// Result returned by [`Http11Send::resume`].
#[derive(Debug)]
pub enum Http11SendResult {
    /// The coroutine has successfully terminated with a non-redirect
    /// response.
    ///
    /// A 3xx response where the `Location` header is absent or
    /// unparseable also arrives here — the caller can inspect
    /// `response.status` if needed.
    Ok {
        /// The response received.
        response: HttpResponse,
        remaining: Vec<u8>,
        /// Whether the server indicated the connection can be reused.
        ///
        /// When `false`, the caller must open a new connection before
        /// sending another request.
        keep_alive: bool,
    },

    WantsRead,
    WantsWrite(Vec<u8>),

    /// The server responded with a 3xx redirect.
    ///
    /// The caller should create a new [`Http11Send`] targeting `url`.
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
        /// Whether the redirect stays on the same scheme, host, and port.
        ///
        /// When `false`, forwarding credentials to the new host
        /// without user consent is inadvisable (RFC 9110 §15.4).
        same_origin: bool,
    },

    /// The coroutine encountered an error.
    Err(Http11SendError),
}

/// Internal state of the [`Http11Send`] coroutine.
#[derive(Debug)]
enum State {
    /// Send the serialized request bytes.
    Headers,
    BodyChunks(Http11ReadChunks),
    BodyLength(usize),
    BodyEof,
}

/// I/O-free coroutine to send an HTTP/1.1 request and receive its response.
///
/// # Example
///
/// ```rust,ignore
/// use std::{io::{Read, Write}, net::TcpStream};
/// use io_http::rfc9112::send::{Http11Send, Http11SendResult};
/// use io_http::rfc9110::request::HttpRequest;
/// use url::Url;
///
/// let url = Url::parse("http://example.com/").unwrap();
/// let request = HttpRequest::get(url.clone())
///     .header("Host", url.host_str().unwrap())
///     .header("Connection", "close");
///
/// let mut stream = TcpStream::connect("example.com:80").unwrap();
/// let mut send = Http11Send::new(request);
/// let mut arg: Option<&[u8]> = None;
/// let mut buf = [0u8; 4096];
///
/// let (response, keep_alive) = loop {
///     match send.resume(arg.take()) {
///         Http11SendResult::Ok { response, keep_alive, .. } => break (response, keep_alive),
///         Http11SendResult::Err(err) => panic!("{err}"),
///         Http11SendResult::WantsRead => {
///             let n = stream.read(&mut buf).unwrap();
///             arg = Some(&buf[..n]);
///         }
///         Http11SendResult::WantsWrite(bytes) => stream.write_all(&bytes).unwrap(),
///         Http11SendResult::WantsRedirect { url: new_url, keep_alive, same_origin, .. } => {
///             if !keep_alive || !same_origin {
///                 stream = TcpStream::connect(new_url.host_str().unwrap()).unwrap();
///             }
///             send = Http11Send::new(HttpRequest::get(new_url));
///             arg = None;
///         }
///     }
/// };
///
/// println!("{}", *response.status);
/// ```
#[derive(Debug)]
pub struct Http11Send {
    request_url: Url,
    state: State,
    wants_read: bool,
    wants_write: Option<Vec<u8>>,
    is_conn_closed: bool,
    buf: Vec<u8>,
    response: ResponseBuilder,
}

impl Http11Send {
    /// Creates a new coroutine that will send the given request and
    /// receive its response.
    pub fn new(req: HttpRequest) -> Self {
        trace!("prepares HTTP/1.1 request to be sent: {req:?}");

        let request_url = req.url.clone();
        let bytes = req.to_http_11_vec();

        Self {
            request_url,
            state: State::Headers,
            wants_read: false,
            wants_write: Some(bytes),
            is_conn_closed: false,
            buf: Vec::new(),
            response: ResponseBuilder::default(),
        }
    }

    /// Advances the coroutine.
    ///
    /// Pass [`None`] when there is no data to provide (initial call,
    /// after a write). Pass `Some(data)` with bytes read from the socket
    /// after a [`Http11SendResult::WantsRead`]. Pass `Some(&[])` to
    /// signal EOF.
    pub fn resume(&mut self, mut arg: Option<&[u8]>) -> Http11SendResult {
        loop {
            if let Some(bytes) = self.wants_write.take() {
                return Http11SendResult::WantsWrite(bytes);
            }

            if mem::take(&mut self.wants_read) {
                return Http11SendResult::WantsRead;
            }

            match &mut self.state {
                State::Headers => {
                    trace!("state: headers");

                    match arg.take() {
                        Some(&[]) => break Http11SendResult::Err(Http11SendError::Eof),
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
                            let err = Http11SendError::ParseResponseHeaders(err);
                            break Http11SendResult::Err(err);
                        }
                    };

                    let mut no_content = false;

                    let is_http10 = matches!(headers.version, Some(0));
                    self.response.version = if is_http10 { HTTP_10 } else { HTTP_11 }.into();

                    if let Some(code) = headers.code {
                        no_content = code == 204 || code == 304;
                        self.response.status = Some(StatusCode(code));
                    }

                    for header in headers.headers {
                        self.response.header(header.name, header.value);
                    }

                    self.is_conn_closed = if let Some(conn) = self.response.get_header(CONNECTION) {
                        conn.eq_ignore_ascii_case("close")
                    } else {
                        // HTTP/1.0 closes connections by default;
                        // HTTP/1.1 keeps them alive.
                        is_http10
                    };

                    trace!("received complete headers: {:?}", self.response);

                    if no_content {
                        let response = mem::take(&mut self.response).build(Vec::new());
                        break self.finish(response, Vec::new());
                    }

                    // remove headers from buffer, leaving pre-read body in place
                    self.buf.drain(..n);

                    // chunked transfer coding is HTTP/1.1 only
                    if !is_http10 {
                        if let Some(enc) = self.response.get_header(TRANSFER_ENCODING) {
                            if enc.eq_ignore_ascii_case("chunked") {
                                let mut c = Http11ReadChunks::default();
                                // Feed any pre-read body bytes to the sub-coroutine
                                // immediately. Subsequent calls pass arg directly,
                                // so Http11Send::buf is no longer used.
                                let preread = mem::take(&mut self.buf);

                                match c.resume(Some(&preread)) {
                                    Http11ReadChunksResult::Ok { body, remaining } => {
                                        let response = mem::take(&mut self.response).build(body);
                                        return self.finish(response, remaining);
                                    }
                                    Http11ReadChunksResult::WantsRead => {
                                        self.state = State::BodyChunks(c);
                                        self.wants_read = true;
                                    }
                                    Http11ReadChunksResult::Err(err) => {
                                        return Http11SendResult::Err(err.into());
                                    }
                                }

                                continue;
                            }
                        }
                    }

                    if let Some(len) = self.response.get_header(CONTENT_LENGTH) {
                        let len = len.trim();

                        let Ok(len) = usize::from_str_radix(len, 10) else {
                            let err = Http11SendError::InvalidContentLength(len.to_owned());
                            return Http11SendResult::Err(err);
                        };

                        self.state = State::BodyLength(len);
                        continue;
                    }

                    self.state = State::BodyEof;
                }
                State::BodyChunks(c) => {
                    trace!("state: body chunks");
                    // Pre-read body was already fed at the Headers→BodyChunks
                    // transition; this arm is always reached after a WantsRead
                    // cycle, so arg always contains fresh bytes.
                    match c.resume(arg.take()) {
                        Http11ReadChunksResult::Ok { body, remaining } => {
                            let response = mem::take(&mut self.response).build(body);
                            return self.finish(response, remaining);
                        }
                        Http11ReadChunksResult::WantsRead => {
                            self.wants_read = true;
                        }
                        Http11ReadChunksResult::Err(err) => {
                            return Http11SendResult::Err(err.into());
                        }
                    }
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
}

impl Http11Send {
    /// Builds the terminal result for the given response, emitting
    /// [`Http11SendResult::WantsRedirect`] when the status is 3xx and
    /// the `Location` header resolves to a valid URL, otherwise
    /// [`Http11SendResult::Ok`].
    fn finish(&self, response: HttpResponse, remaining: Vec<u8>) -> Http11SendResult {
        let keep_alive = !self.is_conn_closed;

        if response.status.is_redirection() {
            if let Some(location) = response.header(LOCATION) {
                if let Ok(url) = self.request_url.join(location) {
                    let same_scheme = self.request_url.scheme() == url.scheme();
                    let same_host = self.request_url.host() == url.host()
                        && self.request_url.port() == url.port();
                    let same_origin = same_scheme && same_host;

                    return Http11SendResult::WantsRedirect {
                        url,
                        response,
                        keep_alive,
                        same_origin,
                    };
                }
            }
        }

        Http11SendResult::Ok {
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
    fn body_chunks() {
        let req = HttpRequest::get("https://example.com".try_into().unwrap());
        let mut coroutine = Http11Send::new(req);
        let mut buf: Option<&[u8]> = None;

        loop {
            match coroutine.resume(buf) {
                Http11SendResult::Ok {
                    response,
                    remaining,
                    keep_alive,
                } => {
                    assert_eq!("HTTP/1.1", response.version);
                    assert_eq!(200, *response.status);
                    assert_eq!(b"hello world", &*response.body);
                    assert_eq!(0, remaining.len());
                    assert_eq!(true, keep_alive);
                    break;
                }
                Http11SendResult::WantsWrite(bytes) => {
                    assert_eq!(bytes, b"GET / HTTP/1.1\r\ncontent-length: 0\r\n\r\n");
                    buf = None;
                }
                Http11SendResult::WantsRead => {
                    buf = Some(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n");
                }
                Http11SendResult::WantsRedirect { .. } => unreachable!("wants redirect"),
                Http11SendResult::Err(err) => unreachable!("{err}"),
            }
        }
    }

    #[test]
    fn body_length() {
        let req = HttpRequest::get("https://example.com".try_into().unwrap());
        let mut coroutine = Http11Send::new(req);
        let mut buf: Option<&[u8]> = None;

        loop {
            match coroutine.resume(buf) {
                Http11SendResult::Ok {
                    response,
                    remaining,
                    keep_alive,
                } => {
                    assert_eq!("HTTP/1.1", response.version);
                    assert_eq!(200, *response.status);
                    assert_eq!(b"hello", &*response.body);
                    assert_eq!(0, remaining.len());
                    assert_eq!(true, keep_alive);
                    break;
                }
                Http11SendResult::WantsWrite(bytes) => {
                    assert_eq!(bytes, b"GET / HTTP/1.1\r\ncontent-length: 0\r\n\r\n");
                    buf = None;
                }
                Http11SendResult::WantsRead => {
                    buf = Some(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello");
                }
                Http11SendResult::WantsRedirect { .. } => unreachable!("wants redirect"),
                Http11SendResult::Err(err) => unreachable!("{err}"),
            }
        }
    }

    #[test]
    fn body_eof() {
        let req = HttpRequest::get("https://example.com".try_into().unwrap());
        let mut coroutine = Http11Send::new(req);
        let mut buf: Option<&[u8]> = None;
        let mut count = 0;

        loop {
            match coroutine.resume(buf) {
                Http11SendResult::Ok {
                    response,
                    remaining,
                    keep_alive,
                } => {
                    assert_eq!("HTTP/1.1", response.version);
                    assert_eq!(200, *response.status);
                    assert_eq!(b"hello world", &*response.body);
                    assert_eq!(0, remaining.len());
                    assert_eq!(true, keep_alive);
                    break;
                }
                Http11SendResult::WantsWrite(bytes) => {
                    assert_eq!(bytes, b"GET / HTTP/1.1\r\ncontent-length: 0\r\n\r\n");
                    buf = None;
                }
                Http11SendResult::WantsRead if count == 0 => {
                    count = 1;
                    buf = Some(b"HTTP/1.1 200 OK\r\n\r\nhello ");
                }
                Http11SendResult::WantsRead if count == 1 => {
                    count = 2;
                    buf = Some(b"world");
                }
                Http11SendResult::WantsRead => {
                    buf = Some(b"");
                }
                Http11SendResult::WantsRedirect { .. } => unreachable!("wants redirect"),
                Http11SendResult::Err(err) => unreachable!("{err}"),
            }
        }
    }
}
