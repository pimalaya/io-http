//! I/O-free coroutine to send HTTP request and receive its response.

use std::mem;

use http::{
    header::{CONNECTION, CONTENT_LENGTH, TRANSFER_ENCODING},
    response::Builder as ResponseBuilder,
    Request, Response, Version,
};
use io_stream::{
    coroutines::{
        read::{ReadStream, ReadStreamError, ReadStreamResult},
        read_exact::{ReadStreamExact, ReadStreamExactError, ReadStreamExactResult},
        read_to_end::{ReadStreamToEnd, ReadStreamToEndError, ReadStreamToEndResult},
        write::{WriteStream, WriteStreamError, WriteStreamResult},
    },
    io::StreamIo,
};
use log::{info, log_enabled, trace, Level};
use thiserror::Error;

use super::read_chunks::{ReadStreamChunks, ReadStreamChunksError, ReadStreamChunksResult};

const CR: u8 = b'\r';
const CRLF: [u8; 2] = [CR, LF];
const LF: u8 = b'\n';
const SP: u8 = b' ';

const CRLF_CRLF: [u8; 4] = [CR, LF, CR, LF];

/// Errors that can occur during the coroutine progression.
#[derive(Debug, Error)]
pub enum SendHttpError {
    /// The coroutine unexpectedly reached the End Of File.
    #[error("Received unexpected EOF")]
    UnexpectedEof,
    /// The HTTP headers could not be parsed.
    #[error("Parse HTTP response headers error: {0}")]
    ParseResponseHeaders(#[source] httparse::Error),

    #[error(transparent)]
    ReadStream(#[from] ReadStreamError),
    #[error(transparent)]
    ReadStreamChunks(#[from] ReadStreamChunksError),
    #[error(transparent)]
    ReadStreamExact(#[from] ReadStreamExactError),
    #[error(transparent)]
    ReadStreamToEnd(#[from] ReadStreamToEndError),
    #[error(transparent)]
    WriteStream(#[from] WriteStreamError),
}

/// Send result returned by the coroutine's resume function.
#[derive(Debug)]
pub enum SendHttpResult {
    /// The coroutine has successfully terminated its execution.
    Ok(SendHttpOk),
    /// The coroutine encountered an error.
    Err(SendHttpError),
    /// The coroutine wants stream I/O.
    Io(StreamIo),
}

/// The coroutine has successfully terminated its execution.
#[derive(Debug)]
pub struct SendHttpOk {
    /// The initial sent request.
    pub request: Request<Vec<u8>>,
    /// The response received.
    pub response: Response<Vec<u8>>,
    /// Is the connection still alive? If not, then a new
    /// connection needs to be established.
    pub keep_alive: bool,
}

/// The internal state of the [`SendHttp`] request coroutine.
#[derive(Debug)]
enum State {
    /// Step for serializin the request into bytes.
    Serialize,

    /// Step for sending the request bytes.
    Send(WriteStream),

    /// Step for receiving response headers.
    ReceiveHeaders { read: ReadStream, headers: Vec<u8> },

    /// Step for receiving the response body as chunks.
    ///
    /// This step is used when the `Transfer-Encoding` response header
    /// is defined and valid.
    ///
    /// Refs: <https://datatracker.ietf.org/doc/html/rfc9112#field.transfer-encoding>
    ReceiveChunkedBody {
        read: ReadStreamChunks,
        response: ResponseBuilder,
    },

    /// Step for receiving the response body when the body size is
    /// fixed.
    ///
    /// This step is used when the `Content-Length` response header is
    /// defined and valid.
    ///
    /// Refs: <https://datatracker.ietf.org/doc/html/rfc9112#body.content-length>
    ReceiveLengthedBody {
        read: ReadStreamExact,
        response: ResponseBuilder,
    },

    /// Step for receiving the response body until EOF.
    ///
    /// This step is used as fallback when the `Transfer-Encoding` or
    /// `Content-Length` response header is undefined or invalid.
    ReceiveBody {
        read: ReadStreamToEnd,
        response: ResponseBuilder,
    },
}

/// I/O-free coroutine to send HTTP request and receive its response.
#[derive(Debug)]
pub struct SendHttp {
    request: Request<Vec<u8>>,
    state: State,
    is_http_10: bool,
    is_conn_closed: bool,
}

impl SendHttp {
    /// Creates a new coroutine to send the given request and receive
    /// its response.
    pub fn new(request: Request<Vec<u8>>) -> Self {
        Self {
            request,
            state: State::Serialize,
            is_http_10: false,
            is_conn_closed: false,
        }
    }

    /// Makes the coroutine progress.
    pub fn resume(&mut self, mut arg: Option<StreamIo>) -> SendHttpResult {
        if arg.is_none() {
            info!("send HTTP request");
        }

        loop {
            match &mut self.state {
                State::Serialize => {
                    let mut bytes = Vec::new();

                    bytes.extend(self.request.method().as_str().as_bytes());
                    bytes.push(SP);
                    bytes.extend(self.request.uri().path().as_bytes());
                    bytes.push(SP);
                    bytes.extend(format!("{:?}", self.request.version()).into_bytes());
                    bytes.extend(CRLF);

                    for (key, val) in self.request.headers() {
                        // skip content length, as it is automatically
                        // generated later on
                        if key == http::header::CONTENT_LENGTH {
                            continue;
                        }

                        bytes.extend(key.as_str().as_bytes());
                        bytes.extend(b": ");
                        bytes.extend(val.as_bytes());
                        bytes.extend(CRLF);
                    }

                    let body = self.request.body();
                    bytes.extend(CONTENT_LENGTH.as_str().as_bytes());
                    bytes.extend(b": ");
                    bytes.extend(body.len().to_string().into_bytes());
                    bytes.extend(CRLF_CRLF);
                    bytes.extend(body);

                    if log_enabled!(Level::Trace) {
                        let req = String::from_utf8_lossy(&bytes);
                        trace!("HTTP request:\n{req}");
                    }

                    let write = WriteStream::new(bytes);
                    self.state = State::Send(write);
                }
                State::Send(write) => {
                    match write.resume(arg.take()) {
                        WriteStreamResult::Ok(_) => (),
                        WriteStreamResult::Err(err) => return SendHttpResult::Err(err.into()),
                        WriteStreamResult::Io(io) => return SendHttpResult::Io(io),
                        WriteStreamResult::Eof => {
                            return SendHttpResult::Err(SendHttpError::UnexpectedEof)
                        }
                    };

                    trace!("resume after sending HTTP response");

                    self.state = State::ReceiveHeaders {
                        read: ReadStream::default(),
                        headers: Vec::new(),
                    };
                }
                State::ReceiveHeaders { read, headers } => {
                    let output = match read.resume(arg.take()) {
                        ReadStreamResult::Ok(output) => output,
                        ReadStreamResult::Err(err) => return SendHttpResult::Err(err.into()),
                        ReadStreamResult::Io(io) => return SendHttpResult::Io(io),
                        ReadStreamResult::Eof => {
                            return SendHttpResult::Err(SendHttpError::UnexpectedEof)
                        }
                    };

                    trace!("resume after receiving partial HTTP response headers");

                    headers.extend(output.bytes());

                    let mut parsed = [httparse::EMPTY_HEADER; 64];
                    let mut parsed = httparse::Response::new(&mut parsed);

                    let n = match parsed.parse(headers) {
                        Ok(httparse::Status::Complete(n)) => n,
                        Ok(httparse::Status::Partial) => {
                            trace!("received incomplete HTTP response headers, need more bytes");
                            read.replace(output.buffer);
                            continue;
                        }
                        Err(err) => {
                            return SendHttpResult::Err(SendHttpError::ParseResponseHeaders(err))
                        }
                    };

                    if log_enabled!(Level::Trace) {
                        let h = String::from_utf8_lossy(&headers[..n]);
                        trace!("HTTP response headers:\n{h}");
                    }

                    let mut response = Response::builder();

                    match parsed.version {
                        Some(0) => {
                            self.is_http_10 = true;
                            response = response.version(Version::HTTP_10);
                        }
                        Some(1) => {
                            response = response.version(Version::HTTP_11);
                        }
                        _ => (),
                    }

                    if let Some(code) = parsed.code {
                        response = response.status(code);
                    }

                    for header in parsed.headers {
                        response = response.header(header.name, header.value);
                    }

                    let body = headers.drain(n..);

                    let Some(headers) = response.headers_ref() else {
                        break SendHttpResult::Ok(SendHttpOk {
                            request: mem::take(&mut self.request),
                            response: response.body(body.collect()).unwrap(),
                            keep_alive: !self.is_http_10,
                        });
                    };

                    if let Some(conn) = headers.get(CONNECTION) {
                        self.is_conn_closed = conn == "close";
                    } else {
                        self.is_conn_closed = self.is_http_10;
                    }

                    if let Some(encoding) = headers.get(TRANSFER_ENCODING) {
                        if encoding == "chunked" {
                            let mut read = ReadStream::with_capacity(output.buffer.capacity());
                            read.replace(output.buffer);

                            let mut read = ReadStreamChunks::new(read);
                            read.extend(body);

                            self.state = State::ReceiveChunkedBody { read, response };
                            continue;
                        }
                    }

                    if let Some(len) = headers.get(CONTENT_LENGTH) {
                        if let Ok(len) = len.to_str() {
                            if let Ok(len) = usize::from_str_radix(len, 10) {
                                let mut read = ReadStreamExact::new(len);
                                read.extend(body);
                                self.state = State::ReceiveLengthedBody { read, response };
                                continue;
                            }
                        }
                    }

                    let mut read = ReadStreamToEnd::new();
                    read.extend(body);
                    self.state = State::ReceiveBody { read, response };
                }
                State::ReceiveChunkedBody { read, response } => {
                    let body = match read.resume(arg.take()) {
                        ReadStreamChunksResult::Ok(body) => body,
                        ReadStreamChunksResult::Err(err) => return SendHttpResult::Err(err.into()),
                        ReadStreamChunksResult::Io(io) => return SendHttpResult::Io(io),
                    };

                    break SendHttpResult::Ok(SendHttpOk {
                        request: mem::take(&mut self.request),
                        response: mem::take(response).body(body).unwrap(),
                        keep_alive: !self.is_conn_closed,
                    });
                }
                State::ReceiveLengthedBody { read, response } => {
                    let body = match read.resume(arg.take()) {
                        ReadStreamExactResult::Ok(body) => body,
                        ReadStreamExactResult::Err(err) => return SendHttpResult::Err(err.into()),
                        ReadStreamExactResult::Io(io) => return SendHttpResult::Io(io),
                    };

                    break SendHttpResult::Ok(SendHttpOk {
                        request: mem::take(&mut self.request),
                        response: mem::take(response).body(body).unwrap(),
                        keep_alive: !self.is_conn_closed,
                    });
                }
                State::ReceiveBody { read, response } => {
                    let body = match read.resume(arg.take()) {
                        ReadStreamToEndResult::Ok(body) => body,
                        ReadStreamToEndResult::Err(err) => return SendHttpResult::Err(err.into()),
                        ReadStreamToEndResult::Io(io) => return SendHttpResult::Io(io),
                    };

                    break SendHttpResult::Ok(SendHttpOk {
                        request: mem::take(&mut self.request),
                        response: mem::take(response).body(body).unwrap(),
                        keep_alive: !self.is_conn_closed,
                    });
                }
            }
        }
    }
}
