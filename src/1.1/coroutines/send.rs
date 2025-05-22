use std::mem;

use http::{
    header::{CONTENT_LENGTH, TRANSFER_ENCODING},
    response::Builder as ResponseBuilder,
    Request, Response, Version,
};
use io_stream::{
    coroutines::{Read, Write},
    Io,
};
use log::{debug, info, log_enabled, trace, Level};

const CR: u8 = b'\r';
const CRLF: [u8; 2] = [CR, LF];
const LF: u8 = b'\n';
const SP: u8 = b' ';

const CRLF_CRLF: [u8; 4] = [CR, LF, CR, LF];

#[derive(Debug)]
pub enum State {
    SerializeRequest,
    SendRequest(Write),
    ReceiveResponseHeaders {
        read: Read,
        headers: Vec<u8>,
    },
    ReceiveResponseBody {
        read: Read,
        response: ResponseBuilder,
        body: Vec<u8>,
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
            state: State::SerializeRequest,
            request,
        }
    }

    pub fn resume(&mut self, mut input: Option<Io>) -> Result<Response<Vec<u8>>, Io> {
        if input.is_none() {
            info!("send HTTP request");
        }

        loop {
            match &mut self.state {
                State::SerializeRequest => {
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
                    self.state = State::SendRequest(write);
                }
                State::SendRequest(write) => {
                    if let Err(io) = write.resume(input.take()) {
                        debug!("break: need I/O to send HTTP request");
                        return Err(io);
                    } else {
                        debug!("resume after HTTP response sent");
                    }

                    self.state = State::ReceiveResponseHeaders {
                        read: Read::default(),
                        headers: Vec::new(),
                    };
                }
                State::ReceiveResponseHeaders { read, headers } => {
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
                        println!("headers: {:?}", String::from_utf8_lossy(output.bytes()));
                        panic!("baaad");
                    }

                    headers.extend(output.bytes());

                    let mut parsed = [httparse::EMPTY_HEADER; 64];
                    let mut parsed = httparse::Response::new(&mut parsed);

                    match parsed.parse(headers) {
                        Ok(httparse::Status::Partial) => {
                            debug!("received incomplete HTTP response headers, need more bytes");
                            read.replace(output.buffer);
                            continue;
                        }
                        Ok(httparse::Status::Complete(n)) => {
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

                            let body = headers.drain(n..).collect();

                            self.state = State::ReceiveResponseBody {
                                read: Read::default(),
                                response,
                                body,
                            };

                            continue;
                        }
                        Err(err) => {
                            // TODO: handle this case
                            panic!("{err}")
                        }
                    };
                }
                State::ReceiveResponseBody {
                    read,
                    response,
                    body,
                } => {
                    let Some(headers) = response.headers_ref() else {
                        let body = mem::take(body);
                        let response = mem::take(response).body(body);
                        break Ok(response.unwrap());
                    };

                    if let Some(encoding) = headers.get(TRANSFER_ENCODING) {
                        if encoding == "chunked" {
                            if body.ends_with(&CRLF_CRLF) {
                                // TODO: decode chunked body properly
                                let body = mem::take(body);
                                let response = mem::take(response).body(body);
                                break Ok(response.unwrap());
                            }
                        }
                    }

                    if let Some(len) = headers.get(CONTENT_LENGTH) {
                        let len = len.to_str().unwrap();
                        let len = usize::from_str_radix(len, 10).unwrap();

                        if body.len() >= len {
                            let body = mem::take(body);
                            let response = mem::take(response).body(body);
                            break Ok(response.unwrap());
                        }
                    }

                    let output = match read.resume(input.take()) {
                        Ok(output) => {
                            debug!("resume after partial HTTP response body received");
                            output
                        }
                        Err(io) => {
                            debug!("break: need I/O to receive HTTP body response");
                            return Err(io);
                        }
                    };

                    if output.bytes_count == 0 {
                        debug!("received 0 bytes, maybe reached EOF?");
                        let body = mem::take(body);
                        let response = mem::take(response).body(body);
                        break Ok(response.unwrap());
                    }

                    body.extend(output.bytes());
                    read.replace(output.buffer);
                    continue;
                }
            }
        }
    }
}
