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

use alloc::{format, string::String, vec, vec::Vec};
use core::mem;

use io_socket::{
    coroutines::{read::*, read_exact::*, read_to_end::*, write::*},
    io::{SocketInput, SocketOutput},
};
use log::{Level, info, log_enabled, trace};
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

const CR: u8 = b'\r';
const CRLF: [u8; 2] = [CR, LF];
const LF: u8 = b'\n';
const SP: u8 = b' ';

const CRLF_CRLF: [u8; 4] = [CR, LF, CR, LF];

/// Errors that can occur during the coroutine progression.
#[derive(Debug, Error)]
pub enum Http11SendError {
    #[error("Received unexpected EOF")]
    UnexpectedEof,
    #[error("Parse HTTP response headers error: {0}")]
    ParseResponseHeaders(httparse::Error),
    #[error(transparent)]
    SocketRead(#[from] SocketReadError),
    #[error(transparent)]
    HttpChunksRead(#[from] HttpChunksReadError),
    #[error(transparent)]
    SocketReadExact(#[from] SocketReadExactError),
    #[error(transparent)]
    SocketReadToEnd(#[from] SocketReadToEndError),
    #[error(transparent)]
    SocketWrite(#[from] SocketWriteError),
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
        /// The request that was sent.
        request: HttpRequest,
        /// The response received.
        response: HttpResponse,
        /// Whether the server indicated the connection can be reused.
        ///
        /// When `false`, the caller must open a new connection before
        /// sending another request.
        keep_alive: bool,
    },

    /// The coroutine needs a socket I/O to be performed.
    Io { input: SocketInput },

    /// The server responded with a 3xx redirect.
    ///
    /// The caller should create a new [`Http11Send`] targeting `url`.
    /// When `!keep_alive || !same_origin`, a new connection must be
    /// opened before sending the next request.
    Redirect {
        /// Resolved redirect target URL (from the `Location` header).
        url: Url,
        /// The request that triggered this redirect.
        request: HttpRequest,
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
    Err { err: Http11SendError },
}

/// Internal state of the [`Http11Send`] coroutine.
#[derive(Debug)]
enum State {
    /// Serialize the request into bytes.
    Serialize,

    /// Send the serialized request bytes.
    Send(SocketWrite),

    /// Receive response headers incrementally.
    ReceiveHeaders { read: SocketRead, headers: Vec<u8> },

    /// Receive the response body using chunked transfer coding.
    ///
    /// Used when the `Transfer-Encoding: chunked` response header is
    /// present.
    ///
    /// Refs: <https://datatracker.ietf.org/doc/html/rfc9112#field.transfer-encoding>
    ReceiveChunkedBody {
        read: HttpChunksRead,
        response: ResponseBuilder,
    },

    /// Receive a fixed-length response body.
    ///
    /// Used when the `Content-Length` response header is present and
    /// valid.
    ///
    /// Refs: <https://datatracker.ietf.org/doc/html/rfc9112#body.content-length>
    ReceiveLengthedBody {
        read: SocketReadExact,
        response: ResponseBuilder,
    },

    /// Receive the response body until EOF.
    ///
    /// Fallback when neither `Transfer-Encoding` nor `Content-Length`
    /// is present or valid.
    ReceiveBody {
        read: SocketReadToEnd,
        response: ResponseBuilder,
    },
}

/// I/O-free coroutine to send an HTTP/1.1 request and receive its response.
///
/// # Example
///
/// ```rust,ignore
/// use std::net::TcpStream;
/// use io_http::rfc9112::send::{Http11Send, Http11SendResult};
/// use io_http::rfc9110::request::HttpRequest;
/// use io_socket::runtimes::std_stream::handle;
/// use url::Url;
///
/// let url = Url::parse("http://example.com/").unwrap();
/// let request = HttpRequest::get(url.clone())
///     .header("Host", url.host_str().unwrap())
///     .header("Connection", "close");
///
/// let mut stream = TcpStream::connect("example.com:80").unwrap();
/// let mut send = Http11Send::new(request);
/// let mut arg = None;
///
/// let (response, keep_alive) = 'outer: loop {
///     match send.resume(arg.take()) {
///         Http11SendResult::Ok { response, keep_alive, .. } => break (response, keep_alive),
///         Http11SendResult::Err { err } => panic!("{err}"),
///         Http11SendResult::Io { input } => arg = Some(handle(&mut stream, input).unwrap()),
///         Http11SendResult::Redirect { url: new_url, keep_alive, same_origin, .. } => {
///             if !keep_alive || !same_origin {
///                 stream = TcpStream::connect(new_url.host_str().unwrap()).unwrap();
///             }
///             send = Http11Send::new(HttpRequest::get(new_url));
///         }
///     }
/// };
///
/// println!("{}", *response.status);
/// ```
#[derive(Debug)]
pub struct Http11Send {
    // Stored as Option because Url is not Default, so we cannot use mem::take
    // on HttpRequest directly. The value is Some for the entire lifetime of the
    // coroutine and taken exactly once in the terminal states.
    request: Option<HttpRequest>,
    state: State,
    is_conn_closed: bool,
}

impl Http11Send {
    /// Creates a new coroutine that will send the given request and
    /// receive its response.
    pub fn new(request: HttpRequest) -> Self {
        Self {
            request: Some(request),
            state: State::Serialize,
            is_conn_closed: false,
        }
    }

    /// Advances the coroutine.
    ///
    /// Pass `None` on the first call. On subsequent calls, pass the
    /// [`SocketOutput`] returned by the runtime after processing the
    /// last emitted [`SocketInput`].
    pub fn resume(&mut self, mut arg: Option<SocketOutput>) -> Http11SendResult {
        if arg.is_none() {
            info!("send HTTP/1.1 request");
        }

        loop {
            match &mut self.state {
                State::Serialize => {
                    let req = self.request.as_ref().unwrap();
                    trace!("HTTP/1.1 request: {req:?}");

                    let mut bytes = Vec::new();

                    bytes.extend(req.method.as_bytes());
                    bytes.push(SP);
                    bytes.extend(req.url.path().as_bytes());

                    if let Some(q) = req.url.query() {
                        bytes.extend(b"?");
                        bytes.extend(q.as_bytes());
                    }

                    bytes.push(SP);
                    bytes.extend(HTTP_11.as_bytes());
                    bytes.extend(CRLF);

                    for (key, val) in &req.headers {
                        // skip content-length, as it is automatically
                        // generated below
                        if key.eq_ignore_ascii_case(CONTENT_LENGTH) {
                            continue;
                        }

                        bytes.extend(key.as_bytes());
                        bytes.extend(b": ");
                        bytes.extend(val.as_bytes());
                        bytes.extend(CRLF);
                    }

                    let body_len = format!("{}", req.body.len());
                    bytes.extend(CONTENT_LENGTH.as_bytes());
                    bytes.extend(b": ");
                    bytes.extend(body_len.as_bytes());
                    bytes.extend(CRLF_CRLF);
                    bytes.extend(&req.body);

                    self.state = State::Send(SocketWrite::new(bytes));
                }
                State::Send(write) => {
                    match write.resume(arg.take()) {
                        SocketWriteResult::Ok { .. } => (),
                        SocketWriteResult::Err { err } => {
                            return Http11SendResult::Err { err: err.into() };
                        }
                        SocketWriteResult::Io { input } => {
                            return Http11SendResult::Io { input };
                        }
                        SocketWriteResult::Eof => {
                            return Http11SendResult::Err {
                                err: Http11SendError::UnexpectedEof,
                            };
                        }
                    };

                    trace!("resume after sending HTTP/1.1 request");

                    self.state = State::ReceiveHeaders {
                        read: SocketRead::default(),
                        headers: Vec::new(),
                    };
                }
                State::ReceiveHeaders { read, headers } => {
                    let (buf, n) = match read.resume(arg.take()) {
                        SocketReadResult::Ok { buf, n } => (buf, n),
                        SocketReadResult::Err { err } => {
                            return Http11SendResult::Err { err: err.into() };
                        }
                        SocketReadResult::Io { input } => {
                            return Http11SendResult::Io { input };
                        }
                        SocketReadResult::Eof => {
                            return Http11SendResult::Err {
                                err: Http11SendError::UnexpectedEof,
                            };
                        }
                    };

                    trace!("resume after receiving partial HTTP/1.1 response headers");

                    headers.extend_from_slice(&buf[..n]);

                    let mut parsed = [httparse::EMPTY_HEADER; 64];
                    let mut parsed = httparse::Response::new(&mut parsed);

                    let n = match parsed.parse(headers) {
                        Ok(httparse::Status::Complete(n)) => n,
                        Ok(httparse::Status::Partial) => {
                            trace!(
                                "received incomplete HTTP/1.1 response headers, need more bytes"
                            );
                            read.replace(buf);
                            continue;
                        }
                        Err(err) => {
                            return Http11SendResult::Err {
                                err: Http11SendError::ParseResponseHeaders(err),
                            };
                        }
                    };

                    if log_enabled!(Level::Trace) {
                        let h = String::from_utf8_lossy(&headers[..n]);
                        trace!("HTTP/1.1 response headers:\n{h}");
                    }

                    let mut response = ResponseBuilder::default();
                    let mut no_content = false;

                    let is_http10 = matches!(parsed.version, Some(0));
                    response.version = if is_http10 { HTTP_10 } else { HTTP_11 }.into();

                    if let Some(code) = parsed.code {
                        no_content = code == 204 || code == 304;
                        response.status = Some(StatusCode(code));
                    }

                    for header in parsed.headers {
                        response.header(header.name, header.value);
                    }

                    let body: Vec<u8> = headers.drain(n..).collect();

                    if let Some(conn) = response.get_header(CONNECTION) {
                        self.is_conn_closed = conn.eq_ignore_ascii_case("close");
                    } else {
                        // HTTP/1.0 closes connections by default;
                        // HTTP/1.1 keeps them alive.
                        self.is_conn_closed = is_http10;
                    }

                    if no_content {
                        break Http11SendResult::Ok {
                            request: self.request.take().unwrap(),
                            response: response.build(vec![]),
                            keep_alive: !self.is_conn_closed,
                        };
                    }

                    // Chunked transfer coding is HTTP/1.1 only (RFC
                    // 9112 §7.1).
                    if !is_http10 {
                        if let Some(enc) = response.get_header(TRANSFER_ENCODING) {
                            if enc.eq_ignore_ascii_case("chunked") {
                                let capacity = buf.capacity();
                                let mut read = SocketRead::with_capacity(capacity);
                                read.replace(buf);

                                let mut read = HttpChunksRead::new(read);
                                read.extend(body);

                                self.state = State::ReceiveChunkedBody { read, response };
                                continue;
                            }
                        }
                    }

                    if let Some(len) = response.get_header(CONTENT_LENGTH) {
                        if let Ok(len) = usize::from_str_radix(len.trim(), 10) {
                            let mut read = SocketReadExact::new(len);
                            read.extend(body);
                            self.state = State::ReceiveLengthedBody { read, response };
                            continue;
                        }
                    }

                    let mut read = SocketReadToEnd::new();
                    read.extend(body);
                    self.state = State::ReceiveBody { read, response };
                }
                State::ReceiveChunkedBody { read, response } => {
                    let body = match read.resume(arg.take()) {
                        HttpChunksReadResult::Ok { body } => body,
                        HttpChunksReadResult::Err { err } => {
                            return Http11SendResult::Err { err: err.into() };
                        }
                        HttpChunksReadResult::Io { input } => {
                            return Http11SendResult::Io { input };
                        }
                    };

                    break finish(
                        self.request.take().unwrap(),
                        mem::take(response).build(body),
                        !self.is_conn_closed,
                    );
                }
                State::ReceiveLengthedBody { read, response } => {
                    let body = match read.resume(arg.take()) {
                        SocketReadExactResult::Ok { buf } => buf,
                        SocketReadExactResult::Err { err } => {
                            return Http11SendResult::Err { err: err.into() };
                        }
                        SocketReadExactResult::Io { input } => {
                            return Http11SendResult::Io { input };
                        }
                    };

                    break finish(
                        self.request.take().unwrap(),
                        mem::take(response).build(body),
                        !self.is_conn_closed,
                    );
                }
                State::ReceiveBody { read, response } => {
                    let body = match read.resume(arg.take()) {
                        SocketReadToEndResult::Ok { buf } => buf,
                        SocketReadToEndResult::Err { err } => {
                            return Http11SendResult::Err { err: err.into() };
                        }
                        SocketReadToEndResult::Io { input } => {
                            return Http11SendResult::Io { input };
                        }
                    };

                    break finish(
                        self.request.take().unwrap(),
                        mem::take(response).build(body),
                        !self.is_conn_closed,
                    );
                }
            }
        }
    }
}

/// Converts a completed request/response pair into the appropriate
/// [`Http11SendResult`].
///
/// If the response is a 3xx with a parseable `Location` header, emits
/// [`Http11SendResult::Redirect`]; otherwise emits
/// [`Http11SendResult::Ok`].
fn finish(request: HttpRequest, response: HttpResponse, keep_alive: bool) -> Http11SendResult {
    if response.status.is_redirection() {
        if let Some(location) = response.header(LOCATION) {
            if let Ok(url) = request.url.join(location) {
                let same_scheme = request.url.scheme() == url.scheme();
                let same_host =
                    request.url.host() == url.host() && request.url.port() == url.port();
                let same_origin = same_scheme && same_host;

                return Http11SendResult::Redirect {
                    url,
                    request,
                    response,
                    keep_alive,
                    same_origin,
                };
            }
        }
    }

    Http11SendResult::Ok {
        request,
        response,
        keep_alive,
    }
}
