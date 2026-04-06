//! I/O-free coroutine to decode a `Transfer-Encoding: chunked`
//! response body (RFC 9112 §7.1).
//!
//! Chunked transfer coding allows a server to stream a response body
//! of unknown length. Each chunk is prefixed with its size in
//! hexadecimal followed by CRLF, and terminated by a zero-size chunk:
//!
//! ```text
//! 5\r\n
//! Hello\r\n
//! 6\r\n
//! World!\r\n
//! 0\r\n
//! \r\n
//! ```
//!
//! This coroutine is driven automatically by
//! [`super::send::Http11Send`] when the response carries
//! `Transfer-Encoding: chunked`. It can also be used standalone when
//! only the body stream is available.

use alloc::{
    string::{String, ToString},
    vec::Vec,
};
use core::mem;

use io_socket::{
    coroutines::{
        read::{SocketRead, SocketReadError, SocketReadResult},
        read_exact::{SocketReadExact, SocketReadExactError, SocketReadExactResult},
    },
    io::{SocketInput, SocketOutput},
};
use memchr::memmem;
use thiserror::Error;

const CR: u8 = b'\r';
const LF: u8 = b'\n';
const CRLF: [u8; 2] = [CR, LF];
const CRLF_CRLF: [u8; 4] = [CR, LF, CR, LF];

/// Errors that can occur during the coroutine progression.
#[derive(Debug, Error)]
pub enum HttpChunksReadError {
    #[error("Received unexpected EOF")]
    UnexpectedEof,
    #[error("Received invalid chunk size: {0}")]
    InvalidChunkSize(String),
    #[error(transparent)]
    SocketRead(#[from] SocketReadError),
    #[error(transparent)]
    SocketReadExact(#[from] SocketReadExactError),
}

/// Result returned by [`HttpChunksRead::resume`].
#[derive(Debug)]
pub enum HttpChunksReadResult {
    /// The coroutine has successfully terminated its execution.
    Ok { body: Vec<u8> },
    /// The coroutine encountered an error.
    Err { err: HttpChunksReadError },
    /// The coroutine needs a socket I/O to be performed.
    Io { input: SocketInput },
}

#[derive(Debug)]
enum State {
    ChunkSize,
    ChunkData { read: SocketReadExact, size: usize },
    Trailer,
}

/// I/O-free coroutine to read an HTTP response body using chunked
/// transfer coding.
#[derive(Debug)]
pub struct HttpChunksRead {
    read: SocketRead,
    state: State,
    buffer: Vec<u8>,
    body: Vec<u8>,
}

impl HttpChunksRead {
    /// Creates a new coroutine from the given [`SocketRead`]
    /// sub-coroutine.
    pub fn new(read: SocketRead) -> Self {
        Self {
            read,
            state: State::ChunkSize,
            buffer: Vec::new(),
            body: Vec::new(),
        }
    }

    /// Extends the inner read buffer with the given bytes.
    pub fn extend(&mut self, bytes: impl IntoIterator<Item = u8>) {
        self.buffer.extend(bytes);
    }

    /// Advances the coroutine.
    ///
    /// Pass `None` on the first call. On subsequent calls, pass the
    /// [`SocketOutput`] returned by the runtime after processing the
    /// last emitted [`SocketInput`].
    pub fn resume(&mut self, mut arg: Option<SocketOutput>) -> HttpChunksReadResult {
        loop {
            match &mut self.state {
                State::ChunkSize => {
                    // chunk = chunk-size [ chunk-extension ] CRLF
                    //         chunk-data CRLF

                    // find chunk CRLF, otherwise read bytes
                    let Some(crlf) = memmem::find(&self.buffer, &CRLF) else {
                        let (buf, n) = match self.read.resume(arg.take()) {
                            SocketReadResult::Ok { buf, n } => (buf, n),
                            SocketReadResult::Err { err } => {
                                return HttpChunksReadResult::Err { err: err.into() };
                            }
                            SocketReadResult::Io { input } => {
                                return HttpChunksReadResult::Io { input };
                            }
                            SocketReadResult::Eof => {
                                return HttpChunksReadResult::Err {
                                    err: HttpChunksReadError::UnexpectedEof,
                                };
                            }
                        };
                        self.buffer.extend_from_slice(&buf[..n]);
                        self.read.replace(buf);
                        continue;
                    };

                    // search for potential chunk extension
                    let ext = memchr::memchr(b';', &self.buffer[..crlf]).unwrap_or(crlf);

                    // extract chunk size
                    let chunk_size = String::from_utf8_lossy(&self.buffer[..ext]);
                    let Ok(chunk_size) = usize::from_str_radix(&chunk_size, 16) else {
                        let chunk_size = chunk_size.to_string();
                        return HttpChunksReadResult::Err {
                            err: HttpChunksReadError::InvalidChunkSize(chunk_size),
                        };
                    };

                    // if chunk size is 0, search for trailer
                    if chunk_size == 0 {
                        // drain till CRLF excluded, so we can easily
                        // look for a double CRLF CRLF afterwards
                        self.buffer.drain(..crlf);
                        self.state = State::Trailer;
                        continue;
                    }

                    // drain till CRLF included
                    self.buffer.drain(..crlf + CRLF.len());

                    // read chunk-data + trailing CRLF as an exact-length read;
                    // pre-seed with already-buffered bytes (but no more than needed
                    // to avoid consuming bytes of the next chunk)
                    let target = chunk_size + CRLF.len();
                    let mut read = SocketReadExact::new(target);
                    let pre_seed = self.buffer.len().min(target);
                    read.extend(self.buffer.drain(..pre_seed));
                    self.state = State::ChunkData {
                        read,
                        size: chunk_size,
                    };
                }
                State::ChunkData { read, size } => {
                    let buf = match read.resume(arg.take()) {
                        SocketReadExactResult::Ok { buf } => buf,
                        SocketReadExactResult::Err { err } => {
                            return HttpChunksReadResult::Err { err: err.into() };
                        }
                        SocketReadExactResult::Io { input } => {
                            return HttpChunksReadResult::Io { input };
                        }
                    };

                    // buf is exactly chunk_data + CRLF; take only chunk_data
                    self.body.extend_from_slice(&buf[..*size]);
                    self.state = State::ChunkSize;
                }
                State::Trailer => {
                    // a double CRLF CRLF means the end of trailer
                    if memmem::find(&self.buffer, &CRLF_CRLF).is_none() {
                        let (buf, n) = match self.read.resume(arg.take()) {
                            SocketReadResult::Ok { buf, n } => (buf, n),
                            SocketReadResult::Err { err } => {
                                return HttpChunksReadResult::Err { err: err.into() };
                            }
                            SocketReadResult::Io { input } => {
                                return HttpChunksReadResult::Io { input };
                            }
                            SocketReadResult::Eof => {
                                return HttpChunksReadResult::Err {
                                    err: HttpChunksReadError::UnexpectedEof,
                                };
                            }
                        };
                        self.buffer.extend_from_slice(&buf[..n]);
                        self.read.replace(buf);
                        continue;
                    };

                    break HttpChunksReadResult::Ok {
                        body: mem::take(&mut self.body),
                    };
                }
            }
        }
    }
}
