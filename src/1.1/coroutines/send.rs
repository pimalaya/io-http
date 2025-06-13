use std::mem;

use http::{
    header::{CONNECTION, CONTENT_LENGTH, TRANSFER_ENCODING},
    response::Builder as ResponseBuilder,
    Request, Response, Version,
};
use io_stream::{
    coroutines::{
        read::{Read, ReadError, ReadResult},
        read_exact::{ReadExact, ReadExactError, ReadExactResult},
        read_to_end::{ReadToEnd, ReadToEndResult},
        write::{Write, WriteError, WriteResult},
    },
    io::StreamIo,
};
use log::{info, log_enabled, trace, Level};
use thiserror::Error;

use super::read_chunks::{ReadChunks, ReadChunksError, ReadChunksResult};

const CR: u8 = b'\r';
const CRLF: [u8; 2] = [CR, LF];
const LF: u8 = b'\n';
const SP: u8 = b' ';

const CRLF_CRLF: [u8; 4] = [CR, LF, CR, LF];

#[derive(Debug)]
pub struct SendOk {
    /// The initial sent request.
    pub request: Request<Vec<u8>>,
    /// The response received.
    pub response: Response<Vec<u8>>,
    /// Is the connection still alive? If not, then a new
    /// connection needs to be established.
    pub keep_alive: bool,
}

#[derive(Debug, Error)]
pub enum SendError {
    #[error("Parse HTTP response headers error: {0}")]
    ParseResponseHeaders(#[source] httparse::Error),
    #[error("Received unexpected EOF")]
    UnexpectedEof,

    #[error(transparent)]
    Read(#[from] ReadError),
    #[error(transparent)]
    ReadChunks(#[from] ReadChunksError),
    #[error(transparent)]
    ReadExact(#[from] ReadExactError),
    #[error(transparent)]
    Write(#[from] WriteError),
}

/// Send result returned by the coroutine's resume function.
#[derive(Debug)]
pub enum SendResult {
    /// The coroutine has successfully terminated its execution.
    Ok(SendOk),
    /// The coroutine encountered an error.
    Err(SendError),
    /// The coroutine wants stream I/O.
    Io(StreamIo),
}

/// The internal state of the [`Send`] HTTP request coroutine.
#[derive(Debug)]
enum State {
    /// Step for serializin the request into bytes.
    Serialize,

    /// Step for sending the request bytes.
    Send(Write),

    /// Step for receiving response headers.
    ReceiveHeaders { read: Read, headers: Vec<u8> },

    /// Step for receiving the response body as chunks.
    ///
    /// This step is used when the `Transfer-Encoding` response header
    /// is defined and valid.
    ///
    /// Refs: <https://datatracker.ietf.org/doc/html/rfc9112#field.transfer-encoding>
    ReceiveChunkedBody {
        read: ReadChunks,
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
        read: ReadExact,
        response: ResponseBuilder,
    },

    /// Step for receiving the response body until EOF.
    ///
    /// This step is used as fallback when the `Transfer-Encoding` or
    /// `Content-Length` response header is undefined or invalid.
    ReceiveBody {
        read: ReadToEnd,
        response: ResponseBuilder,
    },
}

/// The send HTTP request coroutine.
#[derive(Debug)]
pub struct Send {
    request: Request<Vec<u8>>,
    state: State,
    is_http_10: bool,
    is_conn_closed: bool,
}

impl Send {
    pub fn new(request: Request<Vec<u8>>) -> Self {
        Self {
            request,
            state: State::Serialize,
            is_http_10: false,
            is_conn_closed: false,
        }
    }

    pub fn resume(&mut self, mut input: Option<StreamIo>) -> SendResult {
        if input.is_none() {
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

                    let write = Write::new(bytes);
                    self.state = State::Send(write);
                }
                State::Send(write) => {
                    match write.resume(input.take()) {
                        WriteResult::Ok(_) => (),
                        WriteResult::Err(err) => return SendResult::Err(err.into()),
                        WriteResult::Io(io) => return SendResult::Io(io),
                        WriteResult::Eof => return SendResult::Err(SendError::UnexpectedEof),
                    };

                    trace!("resume after sending HTTP response");

                    self.state = State::ReceiveHeaders {
                        read: Read::default(),
                        headers: Vec::new(),
                    };
                }
                State::ReceiveHeaders { read, headers } => {
                    let output = match read.resume(input.take()) {
                        ReadResult::Ok(output) => output,
                        ReadResult::Err(err) => return SendResult::Err(err.into()),
                        ReadResult::Io(io) => return SendResult::Io(io),
                        ReadResult::Eof => return SendResult::Err(SendError::UnexpectedEof),
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
                        Err(err) => return SendResult::Err(SendError::ParseResponseHeaders(err)),
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
                        break SendResult::Ok(SendOk {
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
                            let mut read = Read::with_capacity(output.buffer.capacity());
                            read.replace(output.buffer);

                            let mut read = ReadChunks::new(read);
                            read.extend(body);

                            self.state = State::ReceiveChunkedBody { read, response };
                            continue;
                        }
                    }

                    if let Some(len) = headers.get(CONTENT_LENGTH) {
                        if let Ok(len) = len.to_str() {
                            if let Ok(len) = usize::from_str_radix(len, 10) {
                                let mut read = ReadExact::new(len);
                                read.extend(body);
                                self.state = State::ReceiveLengthedBody { read, response };
                                continue;
                            }
                        }
                    }

                    let mut read = ReadToEnd::new();
                    read.extend(body);
                    self.state = State::ReceiveBody { read, response };
                }
                State::ReceiveChunkedBody { read, response } => {
                    let body = match read.resume(input.take()) {
                        ReadChunksResult::Ok(body) => body,
                        ReadChunksResult::Err(err) => return SendResult::Err(err.into()),
                        ReadChunksResult::Io(io) => return SendResult::Io(io),
                    };

                    break SendResult::Ok(SendOk {
                        request: mem::take(&mut self.request),
                        response: mem::take(response).body(body).unwrap(),
                        keep_alive: !self.is_conn_closed,
                    });
                }
                State::ReceiveLengthedBody { read, response } => {
                    let body = match read.resume(input.take()) {
                        ReadExactResult::Ok(body) => body,
                        ReadExactResult::Err(err) => return SendResult::Err(err.into()),
                        ReadExactResult::Io(io) => return SendResult::Io(io),
                    };

                    break SendResult::Ok(SendOk {
                        request: mem::take(&mut self.request),
                        response: mem::take(response).body(body).unwrap(),
                        keep_alive: !self.is_conn_closed,
                    });
                }
                State::ReceiveBody { read, response } => {
                    let body = match read.resume(input.take()) {
                        ReadToEndResult::Ok(body) => body,
                        ReadToEndResult::Err(err) => return SendResult::Err(err.into()),
                        ReadToEndResult::Io(io) => return SendResult::Io(io),
                    };

                    break SendResult::Ok(SendOk {
                        request: mem::take(&mut self.request),
                        response: mem::take(response).body(body).unwrap(),
                        keep_alive: !self.is_conn_closed,
                    });
                }
            }
        }
    }
}
