//! I/O-free coroutine to send an HTTP request and receive its
//! response (RFC 9112).
//!
//! The coroutine serializes the request, writes it to the socket,
//! then reads and parses the response headers (delegated to
//! [`Http11ReadHeaders`]) and body. Three body-reading strategies are
//! supported, selected automatically from the response headers:
//!
//! | Strategy     | Trigger                      |
//! |--------------|------------------------------|
//! | Chunked      | `Transfer-Encoding: chunked` |
//! | Fixed-length | `Content-Length: <n>`        |
//! | Read-to-EOF  | Neither header present       |

use core::mem;

use alloc::{borrow::ToOwned, string::String, vec::Vec};

use log::trace;
use thiserror::Error;
use url::Url;

use crate::{
    coroutine::*,
    rfc1945::version::HTTP_10,
    rfc9110::{
        headers::{CONTENT_LENGTH, LOCATION, TRANSFER_ENCODING},
        request::HttpRequest,
        response::HttpResponse,
        send::{HttpSendOutput, HttpSendYield},
    },
    rfc9112::{
        chunk::{Http11ReadChunks, Http11ReadChunksError},
        read_headers::{Http11ReadHeaders, Http11ReadHeadersError},
    },
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

impl From<Http11ReadHeadersError> for Http11SendError {
    fn from(err: Http11ReadHeadersError) -> Self {
        match err {
            Http11ReadHeadersError::Eof => Self::Eof,
            Http11ReadHeadersError::ParseResponseHeaders(e) => Self::ParseResponseHeaders(e),
        }
    }
}

/// Internal state of the [`Http11Send`] coroutine.
#[derive(Debug)]
enum State {
    /// Reading and parsing the response head.
    ReadHeaders(Http11ReadHeaders),
    /// Accumulating a chunked-transfer body.
    BodyChunks(Http11ReadChunks),
    /// Accumulating a fixed-length body of `len` bytes.
    BodyLength(usize),
    /// Accumulating body bytes until EOF.
    BodyEof,
}

/// I/O-free coroutine to send an HTTP/1.1 request and receive its response.
///
/// # Example
///
/// ```rust,ignore
/// use std::{io::{Read, Write}, net::TcpStream};
/// use io_http::{
///     coroutine::*,
///     rfc9110::{request::HttpRequest, send::HttpSendYield},
///     rfc9112::send::Http11Send,
/// };
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
///         HttpCoroutineState::Complete(Ok(out)) => break (out.response, out.keep_alive),
///         HttpCoroutineState::Complete(Err(err)) => panic!("{err}"),
///         HttpCoroutineState::Yielded(HttpSendYield::WantsRead) => {
///             let n = stream.read(&mut buf).unwrap();
///             arg = Some(&buf[..n]);
///         }
///         HttpCoroutineState::Yielded(HttpSendYield::WantsWrite(bytes)) => {
///             stream.write_all(&bytes).unwrap();
///         }
///         HttpCoroutineState::Yielded(HttpSendYield::WantsRedirect {
///             url: new_url,
///             keep_alive,
///             same_origin,
///             ..
///         }) => {
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
    wants_write: Option<Vec<u8>>,
    is_conn_closed: bool,
    response: Option<HttpResponse>,
    buf: Vec<u8>,
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
            state: State::ReadHeaders(Http11ReadHeaders::default()),
            wants_write: Some(bytes),
            is_conn_closed: false,
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
    ) -> HttpCoroutineState<HttpSendYield, Result<HttpSendOutput, Http11SendError>> {
        let keep_alive = !self.is_conn_closed;

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
                        unreachable!("Http11ReadHeaders never writes");
                    }
                    HttpCoroutineState::Complete(Err(err)) => {
                        return HttpCoroutineState::Complete(Err(err.into()));
                    }
                    HttpCoroutineState::Complete(Ok(out)) => {
                        let mut response = out.response;
                        let is_http10 = response.version == HTTP_10;
                        self.is_conn_closed = !out.keep_alive;
                        let status = *response.status;

                        // 204 / 304: no body.
                        if status == 204 || status == 304 {
                            return self.finish(response, out.remaining);
                        }

                        // chunked transfer coding is HTTP/1.1 only.
                        if !is_http10 {
                            let chunked = response
                                .header(TRANSFER_ENCODING)
                                .is_some_and(|enc| enc.eq_ignore_ascii_case("chunked"));
                            if chunked {
                                let mut chunks = Http11ReadChunks::default();
                                // Feed any pre-read body bytes immediately so the
                                // sub-coroutine can short-circuit if the whole body
                                // is already buffered.
                                match chunks.resume(Some(&out.remaining)) {
                                    HttpCoroutineState::Complete(Ok(chunk_out)) => {
                                        response.body = chunk_out.body;
                                        return self.finish(response, chunk_out.remaining);
                                    }
                                    HttpCoroutineState::Yielded(HttpYield::WantsRead) => {
                                        self.response = Some(response);
                                        self.state = State::BodyChunks(chunks);
                                        return HttpCoroutineState::Yielded(
                                            HttpSendYield::WantsRead,
                                        );
                                    }
                                    HttpCoroutineState::Yielded(HttpYield::WantsWrite(_)) => {
                                        unreachable!("Http11ReadChunks never writes");
                                    }
                                    HttpCoroutineState::Complete(Err(err)) => {
                                        return HttpCoroutineState::Complete(Err(err.into()));
                                    }
                                }
                            }
                        }

                        if let Some(len_str) = response.header(CONTENT_LENGTH) {
                            let len_str = len_str.trim();
                            let Ok(len) = usize::from_str_radix(len_str, 10) else {
                                let err = Http11SendError::InvalidContentLength(len_str.to_owned());
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
                State::BodyChunks(chunks) => match chunks.resume(arg.take()) {
                    HttpCoroutineState::Complete(Ok(chunk_out)) => {
                        let mut response = self.response.take().expect("response missing");
                        response.body = chunk_out.body;
                        return self.finish(response, chunk_out.remaining);
                    }
                    HttpCoroutineState::Yielded(HttpYield::WantsRead) => {
                        return HttpCoroutineState::Yielded(HttpSendYield::WantsRead);
                    }
                    HttpCoroutineState::Yielded(HttpYield::WantsWrite(_)) => {
                        unreachable!("Http11ReadChunks never writes");
                    }
                    HttpCoroutineState::Complete(Err(err)) => {
                        return HttpCoroutineState::Complete(Err(err.into()));
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
    use crate::rfc9112::send::*;

    #[test]
    fn body_chunks() {
        let req = HttpRequest::get("https://example.com".try_into().unwrap());
        let mut coroutine = Http11Send::new(req);
        let mut buf: Option<&[u8]> = None;

        loop {
            match coroutine.resume(buf) {
                HttpCoroutineState::Complete(Ok(out)) => {
                    assert_eq!("HTTP/1.1", out.response.version);
                    assert_eq!(200, *out.response.status);
                    assert_eq!(b"hello world", &*out.response.body);
                    assert_eq!(0, out.remaining.len());
                    assert_eq!(true, out.keep_alive);
                    break;
                }
                HttpCoroutineState::Yielded(HttpSendYield::WantsWrite(bytes)) => {
                    assert_eq!(bytes, b"GET / HTTP/1.1\r\ncontent-length: 0\r\n\r\n");
                    buf = None;
                }
                HttpCoroutineState::Yielded(HttpSendYield::WantsRead) => {
                    buf = Some(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n");
                }
                HttpCoroutineState::Yielded(HttpSendYield::WantsRedirect { .. }) => {
                    unreachable!("wants redirect");
                }
                HttpCoroutineState::Complete(Err(err)) => unreachable!("{err}"),
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
                HttpCoroutineState::Complete(Ok(out)) => {
                    assert_eq!("HTTP/1.1", out.response.version);
                    assert_eq!(200, *out.response.status);
                    assert_eq!(b"hello", &*out.response.body);
                    assert_eq!(0, out.remaining.len());
                    assert_eq!(true, out.keep_alive);
                    break;
                }
                HttpCoroutineState::Yielded(HttpSendYield::WantsWrite(bytes)) => {
                    assert_eq!(bytes, b"GET / HTTP/1.1\r\ncontent-length: 0\r\n\r\n");
                    buf = None;
                }
                HttpCoroutineState::Yielded(HttpSendYield::WantsRead) => {
                    buf = Some(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello");
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
        let req = HttpRequest::get("https://example.com".try_into().unwrap());
        let mut coroutine = Http11Send::new(req);
        let mut buf: Option<&[u8]> = None;
        let mut count = 0;

        loop {
            match coroutine.resume(buf) {
                HttpCoroutineState::Complete(Ok(out)) => {
                    assert_eq!("HTTP/1.1", out.response.version);
                    assert_eq!(200, *out.response.status);
                    assert_eq!(b"hello world", &*out.response.body);
                    assert_eq!(0, out.remaining.len());
                    assert_eq!(true, out.keep_alive);
                    break;
                }
                HttpCoroutineState::Yielded(HttpSendYield::WantsWrite(bytes)) => {
                    assert_eq!(bytes, b"GET / HTTP/1.1\r\ncontent-length: 0\r\n\r\n");
                    buf = None;
                }
                HttpCoroutineState::Yielded(HttpSendYield::WantsRead) if count == 0 => {
                    count = 1;
                    buf = Some(b"HTTP/1.1 200 OK\r\n\r\nhello ");
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
}
