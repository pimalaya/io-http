use std::mem;

use http::{
    header::{CONTENT_LENGTH, TRANSFER_ENCODING},
    response::Builder as ResponseBuilder,
    Request, Response, Version,
};
use io_stream::{
    coroutines::{Read, ReadExact, ReadToEnd, Write},
    Io,
};
use log::{debug, info, log_enabled, trace, Level};

use super::ChunkedTransferCoding;

const CR: u8 = b'\r';
const CRLF: [u8; 2] = [CR, LF];
const LF: u8 = b'\n';
const SP: u8 = b' ';

const CRLF_CRLF: [u8; 4] = [CR, LF, CR, LF];

#[derive(Debug)]
pub enum State {
    Serialize,
    Send(Write),
    ReceiveHeaders {
        read: Read,
        headers: Vec<u8>,
    },
    ReceiveChunkedBody {
        read: ChunkedTransferCoding,
        response: ResponseBuilder,
    },
    ReceiveLengthedBody {
        read: ReadExact,
        response: ResponseBuilder,
    },
    ReceiveBody {
        read: ReadToEnd,
        response: ResponseBuilder,
    },
}

#[derive(Debug)]
pub struct Send {
    state: State,
    request: Request<Vec<u8>>,
}

impl Send {
    pub fn new(request: Request<Vec<u8>>) -> Self {
        Self {
            state: State::Serialize,
            request,
        }
    }

    pub fn resume(&mut self, mut input: Option<Io>) -> Result<Response<Vec<u8>>, Io> {
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
                    if let Err(io) = write.resume(input.take()) {
                        debug!("break: need I/O to send HTTP request");
                        return Err(io);
                    } else {
                        debug!("resume after HTTP response sent");
                    }

                    self.state = State::ReceiveHeaders {
                        read: Read::default(),
                        headers: Vec::new(),
                    };
                }
                State::ReceiveHeaders { read, headers } => {
                    let output = match read.resume(input.take()) {
                        Ok(output) => {
                            debug!("resume after partial HTTP response headers received");
                            output
                        }
                        Err(io) => {
                            debug!("break: need I/O to receive HTTP response headers");
                            return Err(io);
                        }
                    };

                    if output.bytes_count == 0 {
                        return Err(Io::err("received 0 bytes, reached EOF?"));
                    }

                    headers.extend(output.bytes());

                    let mut parsed = [httparse::EMPTY_HEADER; 64];
                    let mut parsed = httparse::Response::new(&mut parsed);

                    let n = match parsed.parse(headers) {
                        Ok(httparse::Status::Complete(n)) => n,
                        Ok(httparse::Status::Partial) => {
                            debug!("received incomplete HTTP response headers, need more bytes");
                            read.replace(output.buffer);
                            continue;
                        }
                        Err(err) => {
                            let err = format!("parse HTTP headers error: {err}");
                            return Err(Io::err(err));
                        }
                    };

                    if log_enabled!(Level::Trace) {
                        let h = String::from_utf8_lossy(&headers[..n]);
                        trace!("HTTP response headers:\n{h}");
                    }

                    let mut response = Response::builder();

                    match parsed.version {
                        Some(0) => response = response.version(Version::HTTP_10),
                        Some(1) => response = response.version(Version::HTTP_11),
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
                        let response = response.body(body.collect());
                        break Ok(response.unwrap());
                    };

                    if let Some(encoding) = headers.get(TRANSFER_ENCODING) {
                        if encoding == "chunked" {
                            let mut read = Read::with_capacity(output.buffer.capacity());
                            read.replace(output.buffer);

                            let mut read = ChunkedTransferCoding::new(read);
                            read.extend(body);

                            self.state = State::ReceiveChunkedBody { read, response };
                            continue;
                        }
                    }

                    if let Some(len) = headers.get(CONTENT_LENGTH) {
                        if let Ok(len) = len.to_str() {
                            if let Ok(len) = usize::from_str_radix(len, 10) {
                                if len > 0 {
                                    let mut read = ReadExact::new(len);
                                    read.extend(body);
                                    self.state = State::ReceiveLengthedBody { read, response };
                                    continue;
                                }
                            }
                        }
                    }

                    let mut read = ReadToEnd::new();
                    read.extend(body);
                    self.state = State::ReceiveBody { read, response };
                }
                State::ReceiveChunkedBody { read, response } => {
                    let body = read.resume(input.take())?;
                    break Ok(mem::take(response).body(body).unwrap());
                }
                State::ReceiveLengthedBody { read, response } => {
                    let body = read.resume(input.take())?;
                    break Ok(mem::take(response).body(body).unwrap());
                }
                State::ReceiveBody { read, response } => {
                    let body = read.resume(input.take())?;
                    break Ok(mem::take(response).body(body).unwrap());
                }
            }
        }
    }
}
