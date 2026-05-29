//! I/O-free coroutine to decode a `Transfer-Encoding: chunked`
//! response body (RFC 9112 §7.1) one chunk at a time.
//!
//! Unlike [`super::chunk::Http11ReadChunks`], which accumulates every
//! chunk into a single `Vec<u8>` and only returns once the zero-length
//! terminator arrives, this coroutine yields each decoded chunk as
//! soon as its body bytes are available. That makes it suitable for
//! long-lived responses where the caller wants to consume the body
//! incrementally: for example W3C Server-Sent Events, where the
//! server may hold the connection open for hours and emit frames
//! interleaved with keep-alive comments.

use core::mem;

use alloc::{
    string::{String, ToString},
    vec::Vec,
};

use log::trace;
use memchr::memmem;
use thiserror::Error;

use crate::{coroutine::*, rfc9110::chars::CRLF};

/// Errors that can occur during the coroutine progression.
#[derive(Debug, Error)]
pub enum Http11ReadChunksStreamError {
    #[error("Invalid chunk size `{0}`")]
    InvalidChunkSize(String),
}

/// Per-step yield emitted by [`Http11ReadChunksStream`].
///
/// Extends the standard [`HttpYield`] with the [`Self::Frame`] variant
/// so callers can consume each decoded chunk as soon as its body bytes
/// are available.
#[derive(Debug)]
pub enum Http11ReadChunksStreamYield {
    /// Driver should read more bytes from the socket.
    WantsRead,
    /// A complete chunk body has been decoded. The caller should
    /// consume `body` and resume the coroutine to pull the next chunk.
    Frame { body: Vec<u8> },
}

#[derive(Debug, Default)]
enum State {
    #[default]
    ChunkSize,
    ChunkData(usize),
}

/// I/O-free coroutine to read an HTTP response body using chunked
/// transfer coding, yielding each decoded chunk as soon as it
/// arrives.
///
/// On `Complete(Ok(remaining))`, `remaining` carries any bytes already
/// buffered past the zero-length terminator (typically the start of
/// the next pipelined response on the same connection).
#[derive(Debug, Default)]
pub struct Http11ReadChunksStream {
    state: State,
    wants_read: bool,
    done: bool,
    buf: Vec<u8>,
}

impl HttpCoroutine for Http11ReadChunksStream {
    type Yield = Http11ReadChunksStreamYield;
    type Return = Result<Vec<u8>, Http11ReadChunksStreamError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> HttpCoroutineState<Self::Yield, Self::Return> {
        if let Some(data) = arg {
            trace!("resume with arg: {}", String::from_utf8_lossy(data));
            self.buf.extend_from_slice(data);
        }

        loop {
            if self.wants_read {
                self.wants_read = false;
                return HttpCoroutineState::Yielded(Http11ReadChunksStreamYield::WantsRead);
            }

            if self.done {
                let remaining = mem::take(&mut self.buf);
                return HttpCoroutineState::Complete(Ok(remaining));
            }

            match self.state {
                State::ChunkSize => {
                    trace!("state: chunk size");

                    let Some(crlf) = memmem::find(&self.buf, &CRLF) else {
                        self.wants_read = true;
                        continue;
                    };

                    trace!("crlf: {crlf:?}");

                    let ext = match memchr::memchr(b';', &self.buf[..crlf]) {
                        None => crlf,
                        Some(ext) => {
                            let exts = String::from_utf8_lossy(self.buf[ext..crlf].trim_ascii());
                            trace!("ignore extension(s) `{exts}`");
                            ext
                        }
                    };

                    let chunk_size = String::from_utf8_lossy(self.buf[..ext].trim_ascii());

                    let Ok(n) = usize::from_str_radix(&chunk_size, 16) else {
                        let chunk_size = chunk_size.to_string();
                        let err = Http11ReadChunksStreamError::InvalidChunkSize(chunk_size);
                        return HttpCoroutineState::Complete(Err(err));
                    };

                    self.buf.drain(..crlf + CRLF.len());
                    self.state = State::ChunkData(n);
                }
                State::ChunkData(size) if self.buf.len() < size + CRLF.len() => {
                    trace!("state: chunk data {size}");
                    trace!("received incomplete chunk data {}/{size}", self.buf.len());
                    self.wants_read = true;
                    continue;
                }
                State::ChunkData(0) => {
                    trace!("state: terminator chunk");
                    self.buf.drain(..CRLF.len());
                    self.state = State::ChunkSize;
                    self.done = true;
                }
                State::ChunkData(size) => {
                    trace!("state: chunk data {size}");

                    let body: Vec<u8> = self.buf.drain(..size).collect();
                    trace!("drained: {}", String::from_utf8_lossy(&body));
                    self.buf.drain(..CRLF.len());
                    self.state = State::ChunkSize;
                    return HttpCoroutineState::Yielded(Http11ReadChunksStreamYield::Frame {
                        body,
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use crate::rfc9112::chunk_stream::*;

    fn collect(encoded: &str) -> Vec<Vec<u8>> {
        let mut coroutine = Http11ReadChunksStream::default();
        let mut arg: Option<&[u8]> = Some(encoded.as_bytes());
        let mut frames = Vec::new();

        let remaining = loop {
            match coroutine.resume(arg.take()) {
                HttpCoroutineState::Yielded(Http11ReadChunksStreamYield::Frame { body }) => {
                    frames.push(body);
                }
                HttpCoroutineState::Complete(Ok(remaining)) => break remaining,
                HttpCoroutineState::Yielded(Http11ReadChunksStreamYield::WantsRead) => {
                    unreachable!("wants read");
                }
                HttpCoroutineState::Complete(Err(err)) => unreachable!("{err}"),
            }
        };

        assert!(remaining.is_empty(), "unexpected remaining bytes");
        frames
    }

    #[test]
    fn single_chunk() {
        let frames = collect("5\r\nhello\r\n0\r\n\r\n");
        assert_eq!(frames, vec![b"hello".to_vec()]);
    }

    #[test]
    fn two_chunks_yielded_separately() {
        let frames = collect("5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n");
        assert_eq!(frames, vec![b"hello".to_vec(), b" world".to_vec()]);
    }

    #[test]
    fn empty_body() {
        let frames = collect("0\r\n\r\n");
        assert!(frames.is_empty());
    }

    #[test]
    fn ignored_extension() {
        let frames = collect("5;ext\r\nHello\r\n0\r\n\r\n");
        assert_eq!(frames, vec![b"Hello".to_vec()]);
    }

    #[test]
    fn invalid_chunk_size() {
        let mut coroutine = Http11ReadChunksStream::default();
        let buf: Option<&[u8]> = Some(b":\r\n0\r\n\r\n");

        match coroutine.resume(buf) {
            HttpCoroutineState::Complete(Err(Http11ReadChunksStreamError::InvalidChunkSize(s))) => {
                assert_eq!(":", s)
            }
            HttpCoroutineState::Yielded(Http11ReadChunksStreamYield::Frame { body }) => {
                unreachable!("unexpected frame: {body:?}")
            }
            HttpCoroutineState::Complete(Ok(_)) => unreachable!("done"),
            HttpCoroutineState::Yielded(Http11ReadChunksStreamYield::WantsRead) => {
                unreachable!("wants read");
            }
        }
    }

    #[test]
    fn incomplete_chunk_size_then_resume() {
        let mut coroutine = Http11ReadChunksStream::default();
        let mut arg: Option<&[u8]> = Some(b"5\r");
        let mut frames = Vec::new();

        loop {
            match coroutine.resume(arg.take()) {
                HttpCoroutineState::Yielded(Http11ReadChunksStreamYield::Frame { body }) => {
                    frames.push(body);
                }
                HttpCoroutineState::Complete(Ok(remaining)) => {
                    assert!(remaining.is_empty());
                    break;
                }
                HttpCoroutineState::Yielded(Http11ReadChunksStreamYield::WantsRead) => {
                    arg = Some(b"\nHello\r\n0\r\n\r\n");
                }
                HttpCoroutineState::Complete(Err(err)) => unreachable!("{err}"),
            }
        }

        assert_eq!(frames, vec![b"Hello".to_vec()]);
    }

    #[test]
    fn incomplete_chunk_data_then_resume() {
        let mut coroutine = Http11ReadChunksStream::default();
        let mut arg: Option<&[u8]> = Some(b"5\r\nHel");
        let mut frames = Vec::new();

        loop {
            match coroutine.resume(arg.take()) {
                HttpCoroutineState::Yielded(Http11ReadChunksStreamYield::Frame { body }) => {
                    frames.push(body);
                }
                HttpCoroutineState::Complete(Ok(remaining)) => {
                    assert!(remaining.is_empty());
                    break;
                }
                HttpCoroutineState::Yielded(Http11ReadChunksStreamYield::WantsRead) => {
                    arg = Some(b"lo\r\n0\r\n\r\n");
                }
                HttpCoroutineState::Complete(Err(err)) => unreachable!("{err}"),
            }
        }

        assert_eq!(frames, vec![b"Hello".to_vec()]);
    }

    #[test]
    fn remaining_bytes_after_terminator() {
        let mut coroutine = Http11ReadChunksStream::default();
        let mut arg: Option<&[u8]> = Some(b"5\r\nhello\r\n0\r\n\r\nNEXT");
        let mut frames = Vec::new();

        let remaining = loop {
            match coroutine.resume(arg.take()) {
                HttpCoroutineState::Yielded(Http11ReadChunksStreamYield::Frame { body }) => {
                    frames.push(body);
                }
                HttpCoroutineState::Complete(Ok(remaining)) => break remaining,
                HttpCoroutineState::Yielded(Http11ReadChunksStreamYield::WantsRead) => {
                    unreachable!("wants read");
                }
                HttpCoroutineState::Complete(Err(err)) => unreachable!("{err}"),
            }
        };

        assert_eq!(frames, vec![b"hello".to_vec()]);
        assert_eq!(remaining, b"NEXT");
    }
}
