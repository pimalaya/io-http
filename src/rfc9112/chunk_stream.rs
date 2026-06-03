//! I/O-free coroutine decoding a `Transfer-Encoding: chunked` body
//! ([RFC 9112 §7.1]) one chunk at a time; suitable for SSE and other
//! long-lived streams.
//!
//! [RFC 9112 §7.1]: https://www.rfc-editor.org/rfc/rfc9112#section-7.1

use core::{fmt, mem};

use alloc::{
    string::{String, ToString},
    vec::Vec,
};

use log::trace;
use memchr::{memchr, memmem};
use thiserror::Error;

use crate::{coroutine::*, rfc9110::chars::CRLF};

/// Failure causes during the HTTP/1.1 chunked-body streaming flow.
#[derive(Debug, Error)]
pub enum Http11ReadChunksStreamError {
    #[error("HTTP/1.1 read chunks failed: invalid chunk size `{0}`")]
    InvalidChunkSize(String),
}

/// Per-step yield emitted by [`Http11ReadChunksStream`]; adds
/// [`Self::Frame`] to the standard [`HttpYield`] shape.
#[derive(Debug)]
pub enum Http11ReadChunksStreamYield {
    WantsRead,
    Frame { body: Vec<u8> },
}

#[derive(Debug, Default)]
enum State {
    #[default]
    ChunkSize,
    ChunkData(usize),
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ChunkSize => f.write_str("read chunk size"),
            Self::ChunkData(_) => f.write_str("read chunk data"),
        }
    }
}

/// I/O-free streaming chunked-body decoder. `Complete(Ok(remaining))`
/// carries bytes already buffered past the zero-length terminator.
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
            self.buf.extend_from_slice(data);
        }

        loop {
            trace!("http/1.1 stream chunks: {}", self.state);

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
                    let Some(crlf) = memmem::find(&self.buf, &CRLF) else {
                        self.wants_read = true;
                        continue;
                    };

                    let ext = match memchr(b';', &self.buf[..crlf]) {
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
                    trace!("received incomplete chunk data {}/{size}", self.buf.len());
                    self.wants_read = true;
                    continue;
                }
                State::ChunkData(0) => {
                    self.buf.drain(..CRLF.len());
                    self.state = State::ChunkSize;
                    self.done = true;
                }
                State::ChunkData(size) => {
                    let body: Vec<u8> = self.buf.drain(..size).collect();
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

    use super::*;

    #[test]
    fn single_chunk() {
        let frames = collect_all(b"5\r\nhello\r\n0\r\n\r\n");
        assert_eq!(frames, vec![b"hello".to_vec()]);
    }

    #[test]
    fn two_chunks_yielded_separately() {
        let frames = collect_all(b"5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n");
        assert_eq!(frames, vec![b"hello".to_vec(), b" world".to_vec()]);
    }

    #[test]
    fn empty_body() {
        let frames = collect_all(b"0\r\n\r\n");
        assert!(frames.is_empty());
    }

    #[test]
    fn ignored_extension() {
        let frames = collect_all(b"5;ext\r\nHello\r\n0\r\n\r\n");
        assert_eq!(frames, vec![b"Hello".to_vec()]);
    }

    #[test]
    fn invalid_chunk_size() {
        let mut coroutine = Http11ReadChunksStream::default();
        let err = expect_complete_err(&mut coroutine, Some(b":\r\n0\r\n\r\n"));
        let Http11ReadChunksStreamError::InvalidChunkSize(s) = err;
        assert_eq!(s, ":");
    }

    #[test]
    fn incomplete_chunk_size_then_resume() {
        let mut coroutine = Http11ReadChunksStream::default();
        expect_wants_read(&mut coroutine, Some(b"5\r"));
        let body = expect_frame(&mut coroutine, Some(b"\nHello\r\n0\r\n\r\n"));
        assert_eq!(body, b"Hello");
        let remaining = expect_complete_ok(&mut coroutine, None);
        assert!(remaining.is_empty());
    }

    #[test]
    fn remaining_bytes_after_terminator() {
        let mut coroutine = Http11ReadChunksStream::default();
        let body = expect_frame(&mut coroutine, Some(b"5\r\nhello\r\n0\r\n\r\nNEXT"));
        assert_eq!(body, b"hello");
        let remaining = expect_complete_ok(&mut coroutine, None);
        assert_eq!(remaining, b"NEXT");
    }

    // --- utils

    fn collect_all(encoded: &[u8]) -> Vec<Vec<u8>> {
        let mut coroutine = Http11ReadChunksStream::default();
        let mut arg: Option<&[u8]> = Some(encoded);
        let mut frames = Vec::new();

        loop {
            match coroutine.resume(arg.take()) {
                HttpCoroutineState::Yielded(Http11ReadChunksStreamYield::Frame { body }) => {
                    frames.push(body);
                }
                HttpCoroutineState::Complete(Ok(remaining)) => {
                    assert!(remaining.is_empty(), "unexpected remaining bytes");
                    return frames;
                }
                state => panic!("expected Frame or Complete, got {state:?}"),
            }
        }
    }

    fn expect_wants_read(cor: &mut Http11ReadChunksStream, arg: Option<&[u8]>) {
        match cor.resume(arg) {
            HttpCoroutineState::Yielded(Http11ReadChunksStreamYield::WantsRead) => {}
            state => panic!("expected WantsRead, got {state:?}"),
        }
    }

    fn expect_frame(cor: &mut Http11ReadChunksStream, arg: Option<&[u8]>) -> Vec<u8> {
        match cor.resume(arg) {
            HttpCoroutineState::Yielded(Http11ReadChunksStreamYield::Frame { body }) => body,
            state => panic!("expected Frame, got {state:?}"),
        }
    }

    fn expect_complete_ok(cor: &mut Http11ReadChunksStream, arg: Option<&[u8]>) -> Vec<u8> {
        match cor.resume(arg) {
            HttpCoroutineState::Complete(Ok(remaining)) => remaining,
            state => panic!("expected Complete(Ok), got {state:?}"),
        }
    }

    fn expect_complete_err(
        cor: &mut Http11ReadChunksStream,
        arg: Option<&[u8]>,
    ) -> Http11ReadChunksStreamError {
        match cor.resume(arg) {
            HttpCoroutineState::Complete(Err(err)) => err,
            state => panic!("expected Complete(Err), got {state:?}"),
        }
    }
}
