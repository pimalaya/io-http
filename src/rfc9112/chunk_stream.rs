//! I/O-free coroutine decoding a `Transfer-Encoding: chunked` body
//! ([RFC 9112 §7.1]) one chunk at a time; suitable for SSE and other
//! long-lived streams.
//!
//! [RFC 9112 §7.1]: https://www.rfc-editor.org/rfc/rfc9112#section-7.1

use core::mem;

use alloc::{
    string::{String, ToString},
    vec::Vec,
};

use log::{debug, trace};
use thiserror::Error;

use crate::{
    coroutine::*,
    rfc9110::{chars::CRLF, send::HTTP_DEFAULT_MAX_BODY_LEN},
    rfc9112::chunk::HTTP11_MAX_CHUNK_LINE_LEN,
    scan::{crlf_position, position},
};

/// Failure causes during the HTTP/1.1 chunked-body streaming flow.
#[derive(Debug, Error)]
pub enum Http11ChunksReadStreamError {
    /// The chunk size line is not a valid hexadecimal integer.
    #[error("HTTP/1.1 read chunks failed: invalid chunk size `{0}`")]
    InvalidChunkSize(String),
    /// The stream reached EOF before the end of the chunked body.
    #[error("HTTP/1.1 read chunks failed: reached unexpected EOF")]
    Eof,
    /// The chunk size line exceeds [`HTTP11_MAX_CHUNK_LINE_LEN`].
    #[error("HTTP/1.1 read chunks failed: chunk size line exceeds {0} bytes")]
    ChunkLineTooLarge(usize),
    /// A chunk exceeds [`Http11ChunksReadStream::max_chunk_len`].
    #[error("HTTP/1.1 read chunks failed: chunk exceeds {0} bytes")]
    ChunkTooLarge(usize),
    /// The chunk data is not followed by CRLF.
    #[error("HTTP/1.1 read chunks failed: chunk data not followed by CRLF")]
    InvalidChunkTerminator,
    /// A trailer line exceeds [`HTTP11_MAX_CHUNK_LINE_LEN`].
    #[error("HTTP/1.1 read chunks failed: trailer line exceeds {0} bytes")]
    TrailerLineTooLarge(usize),
}

/// Per-step yield emitted by [`Http11ChunksReadStream`]; adds
/// [`Self::Frame`] to the standard [`HttpYield`] shape.
#[derive(Debug)]
pub enum Http11ChunksReadStreamYield {
    /// The coroutine wants bytes read from the stream and handed back
    /// on the next resume.
    WantsRead,
    /// One decoded chunk, yielded as soon as its bytes are available.
    Frame {
        /// The decoded chunk body.
        body: Vec<u8>,
    },
}

#[derive(Debug, Default)]
enum State {
    #[default]
    ChunkSize,
    ChunkData(usize),
    Trailers,
}

/// I/O-free streaming chunked-body decoder. `Complete(Ok(remaining))`
/// carries bytes already buffered past the trailer section, whose
/// fields are read and dropped.
#[derive(Debug)]
pub struct Http11ChunksReadStream {
    /// Maximum size in bytes of a single chunk. A chunk announcing more
    /// fails the read with [`Http11ChunksReadStreamError::ChunkTooLarge`]
    /// before any of it is buffered. The stream itself is unbounded.
    /// Defaults to [`HTTP_DEFAULT_MAX_BODY_LEN`].
    pub max_chunk_len: usize,
    state: State,
    wants_read: bool,
    done: bool,
    buf: Vec<u8>,
    /// Where the bytes not consumed yet start in `buf`.
    start: usize,
}

impl Default for Http11ChunksReadStream {
    fn default() -> Self {
        Self {
            max_chunk_len: HTTP_DEFAULT_MAX_BODY_LEN,
            state: State::default(),
            wants_read: false,
            done: false,
            buf: Vec::new(),
            start: 0,
        }
    }
}

impl HttpCoroutine for Http11ChunksReadStream {
    type Yield = Http11ChunksReadStreamYield;
    type Return = Result<Vec<u8>, Http11ChunksReadStreamError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> HttpCoroutineState<Self::Yield, Self::Return> {
        match arg {
            Some(&[]) => {
                return HttpCoroutineState::Complete(Err(Http11ChunksReadStreamError::Eof));
            }
            Some(data) => {
                self.buf.drain(..self.start);
                self.start = 0;
                self.buf.extend_from_slice(data);
            }
            None => {}
        }

        loop {
            if self.wants_read {
                self.wants_read = false;
                return HttpCoroutineState::Yielded(Http11ChunksReadStreamYield::WantsRead);
            }

            if self.done {
                self.buf.drain(..self.start);
                self.start = 0;

                let remaining = mem::take(&mut self.buf);
                return HttpCoroutineState::Complete(Ok(remaining));
            }

            let rest = &self.buf[self.start..];

            match self.state {
                State::ChunkSize => {
                    let line_window = rest.len().min(HTTP11_MAX_CHUNK_LINE_LEN + CRLF.len());

                    let Some(crlf) = crlf_position(&rest[..line_window]) else {
                        if line_window == HTTP11_MAX_CHUNK_LINE_LEN + CRLF.len() {
                            let limit = HTTP11_MAX_CHUNK_LINE_LEN;
                            let err = Http11ChunksReadStreamError::ChunkLineTooLarge(limit);
                            return HttpCoroutineState::Complete(Err(err));
                        }

                        self.wants_read = true;
                        continue;
                    };

                    let ext = match position(&rest[..crlf], |b| b == b';') {
                        None => crlf,
                        Some(ext) => {
                            let exts = String::from_utf8_lossy(rest[ext..crlf].trim_ascii());
                            trace!("ignore extension(s) `{exts}`");
                            ext
                        }
                    };

                    let chunk_size = String::from_utf8_lossy(rest[..ext].trim_ascii());

                    let is_hex = chunk_size.bytes().all(|b| b.is_ascii_hexdigit());

                    let Some(n) = is_hex
                        .then(|| usize::from_str_radix(&chunk_size, 16).ok())
                        .flatten()
                    else {
                        let chunk_size = chunk_size.to_string();
                        let err = Http11ChunksReadStreamError::InvalidChunkSize(chunk_size);
                        return HttpCoroutineState::Complete(Err(err));
                    };

                    // NOTE: the checked addition keeps `n + CRLF`
                    // addressable even when the limit is `usize::MAX`.
                    if n > self.max_chunk_len || n.checked_add(CRLF.len()).is_none() {
                        let err = Http11ChunksReadStreamError::ChunkTooLarge(self.max_chunk_len);
                        return HttpCoroutineState::Complete(Err(err));
                    }

                    self.start += crlf + CRLF.len();

                    if n == 0 {
                        debug!("reading trailers");
                        self.state = State::Trailers;
                    } else {
                        debug!("reading chunk of {n} bytes");
                        self.state = State::ChunkData(n);
                    }
                }
                State::ChunkData(size) if rest.len() < size + CRLF.len() => {
                    trace!("received incomplete chunk data {}/{size}", rest.len());
                    self.wants_read = true;
                    continue;
                }
                State::Trailers => {
                    let line_window = rest.len().min(HTTP11_MAX_CHUNK_LINE_LEN + CRLF.len());

                    let Some(crlf) = crlf_position(&rest[..line_window]) else {
                        if line_window == HTTP11_MAX_CHUNK_LINE_LEN + CRLF.len() {
                            let limit = HTTP11_MAX_CHUNK_LINE_LEN;
                            let err = Http11ChunksReadStreamError::TrailerLineTooLarge(limit);
                            return HttpCoroutineState::Complete(Err(err));
                        }

                        self.wants_read = true;
                        continue;
                    };

                    self.start += crlf + CRLF.len();

                    if crlf == 0 {
                        self.done = true;
                    } else {
                        trace!("ignore trailer field of {crlf} bytes");
                    }
                }
                State::ChunkData(size) => {
                    if rest[size..size + CRLF.len()] != CRLF {
                        let err = Http11ChunksReadStreamError::InvalidChunkTerminator;
                        return HttpCoroutineState::Complete(Err(err));
                    }

                    let body = rest[..size].to_vec();
                    self.start += size + CRLF.len();
                    self.state = State::ChunkSize;
                    return HttpCoroutineState::Yielded(Http11ChunksReadStreamYield::Frame {
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
        let mut coroutine = Http11ChunksReadStream::default();
        let err = expect_complete_err(&mut coroutine, Some(b":\r\n0\r\n\r\n"));
        let Http11ChunksReadStreamError::InvalidChunkSize(s) = err else {
            panic!("expected InvalidChunkSize, got {err:?}");
        };
        assert_eq!(s, ":");
    }

    #[test]
    fn max_usize_chunk_size_errors() {
        let mut coroutine = Http11ChunksReadStream {
            max_chunk_len: usize::MAX,
            ..Http11ChunksReadStream::default()
        };
        let err = expect_complete_err(&mut coroutine, Some(b"FFFFFFFFFFFFFFFF\r\n"));
        assert!(matches!(err, Http11ChunksReadStreamError::ChunkTooLarge(_)));
    }

    #[test]
    fn chunk_over_limit_errors() {
        let mut coroutine = Http11ChunksReadStream {
            max_chunk_len: 4,
            ..Http11ChunksReadStream::default()
        };
        let err = expect_complete_err(&mut coroutine, Some(b"5\r\nhello\r\n"));
        assert!(matches!(err, Http11ChunksReadStreamError::ChunkTooLarge(4)));
    }

    #[test]
    fn chunk_line_over_limit_errors() {
        let mut coroutine = Http11ChunksReadStream::default();
        let line = [b'0'; HTTP11_MAX_CHUNK_LINE_LEN + 2];
        let err = expect_complete_err(&mut coroutine, Some(&line));
        assert!(matches!(
            err,
            Http11ChunksReadStreamError::ChunkLineTooLarge(_)
        ));
    }

    #[test]
    fn incomplete_chunk_size_then_resume() {
        let mut coroutine = Http11ChunksReadStream::default();
        expect_wants_read(&mut coroutine, Some(b"5\r"));
        let body = expect_frame(&mut coroutine, Some(b"\nHello\r\n0\r\n\r\n"));
        assert_eq!(body, b"Hello");
        let remaining = expect_complete_ok(&mut coroutine, None);
        assert!(remaining.is_empty());
    }

    #[test]
    fn remaining_bytes_after_terminator() {
        let mut coroutine = Http11ChunksReadStream::default();
        let body = expect_frame(&mut coroutine, Some(b"5\r\nhello\r\n0\r\n\r\nNEXT"));
        assert_eq!(body, b"hello");
        let remaining = expect_complete_ok(&mut coroutine, None);
        assert_eq!(remaining, b"NEXT");
    }

    fn collect_all(encoded: &[u8]) -> Vec<Vec<u8>> {
        let mut coroutine = Http11ChunksReadStream::default();
        let mut arg: Option<&[u8]> = Some(encoded);
        let mut frames = Vec::new();

        loop {
            match coroutine.resume(arg.take()) {
                HttpCoroutineState::Yielded(Http11ChunksReadStreamYield::Frame { body }) => {
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

    fn expect_wants_read(cor: &mut Http11ChunksReadStream, arg: Option<&[u8]>) {
        match cor.resume(arg) {
            HttpCoroutineState::Yielded(Http11ChunksReadStreamYield::WantsRead) => {}
            state => panic!("expected WantsRead, got {state:?}"),
        }
    }

    fn expect_frame(cor: &mut Http11ChunksReadStream, arg: Option<&[u8]>) -> Vec<u8> {
        match cor.resume(arg) {
            HttpCoroutineState::Yielded(Http11ChunksReadStreamYield::Frame { body }) => body,
            state => panic!("expected Frame, got {state:?}"),
        }
    }

    fn expect_complete_ok(cor: &mut Http11ChunksReadStream, arg: Option<&[u8]>) -> Vec<u8> {
        match cor.resume(arg) {
            HttpCoroutineState::Complete(Ok(remaining)) => remaining,
            state => panic!("expected Complete(Ok), got {state:?}"),
        }
    }

    fn expect_complete_err(
        cor: &mut Http11ChunksReadStream,
        arg: Option<&[u8]>,
    ) -> Http11ChunksReadStreamError {
        match cor.resume(arg) {
            HttpCoroutineState::Complete(Err(err)) => err,
            state => panic!("expected Complete(Err), got {state:?}"),
        }
    }
}
