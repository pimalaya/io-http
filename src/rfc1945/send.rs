//! I/O-free coroutine to send an HTTP request and receive its
//! response (RFC 1945).
//!
//! The coroutine serializes the request, hands the caller the bytes
//! to write, then collects bytes from the caller until the response
//! headers (delegated to [`Http11ReadHeaders`]) and body are complete.
//! Two body-reading strategies are supported, selected automatically
//! from the response headers:
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

use log::trace;
use thiserror::Error;
use url::Url;

use crate::{
    coroutine::*,
    rfc9110::{
        headers::{CONTENT_LENGTH, LOCATION},
        request::HttpRequest,
        response::HttpResponse,
        send::{HttpSendOutput, HttpSendYield},
    },
    rfc9112::read_headers::{Http11ReadHeaders, Http11ReadHeadersError},
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

impl From<Http11ReadHeadersError> for Http10SendError {
    fn from(err: Http11ReadHeadersError) -> Self {
        match err {
            Http11ReadHeadersError::Eof => Self::Eof,
            Http11ReadHeadersError::ParseResponseHeaders(e) => Self::ParseResponseHeaders(e),
        }
    }
}

/// Internal state of the [`Http10Send`] coroutine.
#[derive(Debug)]
enum State {
    /// Reading and parsing the response head.
    ReadHeaders(Http11ReadHeaders),
    /// Accumulating a fixed-length body of `len` bytes.
    BodyLength(usize),
    /// Accumulating body bytes until EOF.
    BodyEof,
}

/// I/O-free coroutine to send an HTTP/1.0 request and receive its response.
///
/// # Example
///
/// ```rust,ignore
/// use std::{io::{Read, Write}, net::TcpStream};
/// use io_http::{
///     coroutine::*,
///     rfc1945::send::Http10Send,
///     rfc9110::{request::HttpRequest, send::HttpSendYield},
/// };
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
///         HttpCoroutineState::Complete(Ok(out)) => break out.response,
///         HttpCoroutineState::Complete(Err(err)) => panic!("{err}"),
///         HttpCoroutineState::Yielded(HttpSendYield::WantsWrite(bytes)) => {
///             stream.write_all(&bytes).unwrap();
///         }
///         HttpCoroutineState::Yielded(HttpSendYield::WantsRead) => {
///             let n = stream.read(&mut buf).unwrap();
///             arg = Some(&buf[..n]);
///         }
///         HttpCoroutineState::Yielded(HttpSendYield::WantsRedirect { .. }) => unimplemented!(),
///     }
/// };
///
/// println!("{}", *response.status);
/// ```
#[derive(Debug)]
pub struct Http10Send {
    request_url: Url,
    state: State,
    wants_write: Option<Vec<u8>>,
    keep_alive: bool,
    response: Option<HttpResponse>,
    buf: Vec<u8>,
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
            state: State::ReadHeaders(Http11ReadHeaders::default()),
            wants_write: Some(bytes),
            keep_alive: false,
            response: None,
            buf: Vec::new(),
        }
    }

    /// Builds the terminal state for the given response, emitting
    /// [`HttpSendYield::WantsRedirect`] when the status is 3xx and the
    /// `Location` header resolves to a valid URL, otherwise
    /// `Complete(Ok(HttpSendOutput { … }))`.
    fn finish(
        &self,
        response: HttpResponse,
        remaining: Vec<u8>,
    ) -> HttpCoroutineState<HttpSendYield, Result<HttpSendOutput, Http10SendError>> {
        let keep_alive = self.keep_alive;

        if response.status.is_redirection() {
            if let Some(location) = response.header(LOCATION) {
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

impl HttpCoroutine for Http10Send {
    type Yield = HttpSendYield;
    type Return = Result<HttpSendOutput, Http10SendError>;

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
                        unreachable!("Http11ReadHeaders never writes");
                    }
                    HttpCoroutineState::Complete(Err(err)) => {
                        return HttpCoroutineState::Complete(Err(err.into()));
                    }
                    HttpCoroutineState::Complete(Ok(out)) => {
                        let response = out.response;
                        self.keep_alive = out.keep_alive;
                        let status = *response.status;

                        // 204 / 304: no body.
                        if status == 204 || status == 304 {
                            return self.finish(response, out.remaining);
                        }

                        if let Some(len_str) = response.header(CONTENT_LENGTH) {
                            let len_str = len_str.trim();
                            let Ok(len) = usize::from_str_radix(len_str, 10) else {
                                let err = Http10SendError::InvalidContentLength(len_str.to_owned());
                                return HttpCoroutineState::Complete(Err(err));
                            };
                            self.buf = out.remaining;
                            self.response = Some(response);
                            self.state = State::BodyLength(len);
                            continue;
                        }

                        self.buf = out.remaining;
                        self.response = Some(response);
                        self.state = State::BodyEof;
                    }
                },
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
    use crate::rfc1945::send::*;

    #[test]
    fn body_length() {
        let req = HttpRequest::get("http://example.com".try_into().unwrap());
        let mut coroutine = Http10Send::new(req);
        let mut buf: Option<&[u8]> = None;

        loop {
            match coroutine.resume(buf) {
                HttpCoroutineState::Complete(Ok(out)) => {
                    assert_eq!("HTTP/1.0", out.response.version);
                    assert_eq!(200, *out.response.status);
                    assert_eq!(b"hello", &*out.response.body);
                    assert_eq!(0, out.remaining.len());
                    assert_eq!(false, out.keep_alive);
                    break;
                }
                HttpCoroutineState::Yielded(HttpSendYield::WantsWrite(bytes)) => {
                    assert_eq!(bytes, b"GET / HTTP/1.0\r\ncontent-length: 0\r\n\r\n");
                    buf = None;
                }
                HttpCoroutineState::Yielded(HttpSendYield::WantsRead) => {
                    buf = Some(b"HTTP/1.0 200 OK\r\nContent-Length: 5\r\n\r\nhello");
                }
                HttpCoroutineState::Yielded(HttpSendYield::WantsRedirect { .. }) => {
                    unreachable!("wants redirect");
                }
                HttpCoroutineState::Complete(Err(err)) => unreachable!("{err}"),
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
                HttpCoroutineState::Complete(Ok(out)) => {
                    assert_eq!("HTTP/1.0", out.response.version);
                    assert_eq!(200, *out.response.status);
                    assert_eq!(b"hello world", &*out.response.body);
                    assert_eq!(0, out.remaining.len());
                    assert_eq!(false, out.keep_alive);
                    break;
                }
                HttpCoroutineState::Yielded(HttpSendYield::WantsWrite(bytes)) => {
                    assert_eq!(bytes, b"GET / HTTP/1.0\r\ncontent-length: 0\r\n\r\n");
                    buf = None;
                }
                HttpCoroutineState::Yielded(HttpSendYield::WantsRead) if count == 0 => {
                    count = 1;
                    buf = Some(b"HTTP/1.0 200 OK\r\n\r\nhello ");
                }
                HttpCoroutineState::Yielded(HttpSendYield::WantsRead) if count == 1 => {
                    count = 2;
                    buf = Some(b"world");
                }
                HttpCoroutineState::Yielded(HttpSendYield::WantsRead) => {
                    buf = Some(b"");
                }
                HttpCoroutineState::Yielded(HttpSendYield::WantsRedirect { .. }) => {
                    unreachable!("wants redirect");
                }
                HttpCoroutineState::Complete(Err(err)) => unreachable!("{err}"),
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
                HttpCoroutineState::Complete(Ok(out)) => {
                    assert!(out.keep_alive);
                    break;
                }
                HttpCoroutineState::Yielded(HttpSendYield::WantsWrite(_)) => buf = None,
                HttpCoroutineState::Yielded(HttpSendYield::WantsRead) => {
                    buf = Some(
                        b"HTTP/1.0 200 OK\r\nConnection: keep-alive\r\nContent-Length: 0\r\n\r\n",
                    );
                }
                HttpCoroutineState::Yielded(HttpSendYield::WantsRedirect { .. }) => {
                    unreachable!("wants redirect");
                }
                HttpCoroutineState::Complete(Err(err)) => unreachable!("{err}"),
            }
        }
    }
}
