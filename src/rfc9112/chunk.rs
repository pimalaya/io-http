//! I/O-free coroutine decoding a `Transfer-Encoding: chunked` body
//! ([RFC 9112 §7.1]) into a single buffer. For incremental consumption
//! use [`crate::rfc9112::chunk_stream::Http11ChunksReadStream`].
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
    rfc9112::read_headers::HTTP_DEFAULT_MAX_HEADERS_LEN,
    scan::{crlf_position, position},
};

/// Maximum size in bytes of a chunk size line, extensions included
/// and CRLF excluded: 4 KiB.
pub const HTTP11_MAX_CHUNK_LINE_LEN: usize = 4 * 1024;

/// Failure causes during the HTTP/1.1 chunked-body read flow.
#[derive(Debug, Error)]
pub enum Http11ChunksReadError {
    /// The chunk size line is not a valid hexadecimal integer.
    #[error("HTTP/1.1 read chunks failed: invalid chunk size `{0}`")]
    InvalidChunkSize(String),
    /// The stream reached EOF before the end of the chunked body.
    #[error("HTTP/1.1 read chunks failed: reached unexpected EOF")]
    Eof,
    /// The chunk size line exceeds [`HTTP11_MAX_CHUNK_LINE_LEN`].
    #[error("HTTP/1.1 read chunks failed: chunk size line exceeds {0} bytes")]
    ChunkLineTooLarge(usize),
    /// The decoded body would exceed [`Http11ChunksRead::max_body_len`].
    #[error("HTTP/1.1 read chunks failed: body exceeds {0} bytes")]
    BodyTooLarge(usize),
    /// The chunk data is not followed by CRLF.
    #[error("HTTP/1.1 read chunks failed: chunk data not followed by CRLF")]
    InvalidChunkTerminator,
    /// The trailer section exceeds [`Http11ChunksRead::max_trailers_len`].
    #[error("HTTP/1.1 read chunks failed: trailers exceed {0} bytes")]
    TrailersTooLarge(usize),
}

/// Terminal output of [`Http11ChunksRead`].
#[derive(Debug)]
pub struct Http11ChunksReadOutput {
    /// The decoded body, all chunks concatenated.
    pub body: Vec<u8>,
    /// Bytes already buffered past the end of the trailer section.
    pub remaining: Vec<u8>,
}

#[derive(Debug, Default)]
enum State {
    #[default]
    ChunkSize,
    /// The bytes of chunk data still to read.
    ChunkData(usize),
    ChunkEnd,
    Trailers,
}

/// I/O-free coroutine to read an HTTP response body using chunked
/// transfer coding. Trailer fields are read and dropped.
#[derive(Debug)]
pub struct Http11ChunksRead {
    /// Maximum size in bytes of the decoded body, all chunks
    /// concatenated. A chunk announcing more fails the read with
    /// [`Http11ChunksReadError::BodyTooLarge`] before any of it is
    /// buffered. Defaults to [`HTTP_DEFAULT_MAX_BODY_LEN`].
    pub max_body_len: usize,
    /// Maximum size in bytes of the trailer section, line ends
    /// excluded. A server exceeding it fails the read with
    /// [`Http11ChunksReadError::TrailersTooLarge`]. Defaults to
    /// [`HTTP_DEFAULT_MAX_HEADERS_LEN`].
    pub max_trailers_len: usize,
    state: State,
    wants_read: bool,
    last_chunk: bool,
    buf: Vec<u8>,
    /// Where the bytes not consumed yet start in `buf`.
    start: usize,
    body: Vec<u8>,
    trailers_len: usize,
}

impl Default for Http11ChunksRead {
    fn default() -> Self {
        Self {
            max_body_len: HTTP_DEFAULT_MAX_BODY_LEN,
            max_trailers_len: HTTP_DEFAULT_MAX_HEADERS_LEN,
            state: State::default(),
            wants_read: false,
            last_chunk: false,
            buf: Vec::new(),
            start: 0,
            body: Vec::new(),
            trailers_len: 0,
        }
    }
}

impl HttpCoroutine for Http11ChunksRead {
    type Yield = HttpYield;
    type Return = Result<Http11ChunksReadOutput, Http11ChunksReadError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> HttpCoroutineState<Self::Yield, Self::Return> {
        match arg {
            Some(&[]) => return HttpCoroutineState::Complete(Err(Http11ChunksReadError::Eof)),
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
                return HttpCoroutineState::Yielded(HttpYield::WantsRead);
            }

            if self.last_chunk {
                self.buf.drain(..self.start);
                self.start = 0;

                let body = mem::take(&mut self.body);
                let remaining = mem::take(&mut self.buf);
                return HttpCoroutineState::Complete(Ok(Http11ChunksReadOutput {
                    body,
                    remaining,
                }));
            }

            let rest = &self.buf[self.start..];

            match self.state {
                State::ChunkSize => {
                    let line_window = rest.len().min(HTTP11_MAX_CHUNK_LINE_LEN + CRLF.len());

                    let Some(crlf) = crlf_position(&rest[..line_window]) else {
                        if line_window == HTTP11_MAX_CHUNK_LINE_LEN + CRLF.len() {
                            let err =
                                Http11ChunksReadError::ChunkLineTooLarge(HTTP11_MAX_CHUNK_LINE_LEN);
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
                        let err = Http11ChunksReadError::InvalidChunkSize(chunk_size);
                        return HttpCoroutineState::Complete(Err(err));
                    };

                    // NOTE: the checked addition keeps `n + CRLF`
                    // addressable even when the limit is `usize::MAX`.
                    let room = self.max_body_len.saturating_sub(self.body.len());
                    if n > room || n.checked_add(CRLF.len()).is_none() {
                        let err = Http11ChunksReadError::BodyTooLarge(self.max_body_len);
                        return HttpCoroutineState::Complete(Err(err));
                    }

                    self.start += crlf + CRLF.len();

                    if n == 0 {
                        debug!("reading trailers");
                        self.state = State::Trailers;
                    } else {
                        debug!("reading chunk of {n} bytes");
                        // NOTE: one block for the chunk, growth as usual
                        // when the allocator refuses it
                        let _ = self.body.try_reserve(n);
                        self.state = State::ChunkData(n);
                    }
                }
                State::ChunkData(left) => {
                    let taken = rest.len().min(left);

                    self.body.extend_from_slice(&rest[..taken]);
                    self.start += taken;

                    if taken < left {
                        trace!("received incomplete chunk data, {} left", left - taken);
                        self.state = State::ChunkData(left - taken);
                        self.wants_read = true;
                        continue;
                    }

                    self.state = State::ChunkEnd;
                }
                State::ChunkEnd if rest.len() < CRLF.len() => {
                    self.wants_read = true;
                    continue;
                }
                State::ChunkEnd => {
                    if rest[..CRLF.len()] != CRLF {
                        let err = Http11ChunksReadError::InvalidChunkTerminator;
                        return HttpCoroutineState::Complete(Err(err));
                    }

                    self.start += CRLF.len();
                    self.state = State::ChunkSize;
                }
                State::Trailers => {
                    let room = self.max_trailers_len.saturating_sub(self.trailers_len);
                    let room = room.saturating_add(CRLF.len());
                    let line_window = rest.len().min(room);

                    let Some(crlf) = crlf_position(&rest[..line_window]) else {
                        if line_window == room {
                            let limit = self.max_trailers_len;
                            let err = Http11ChunksReadError::TrailersTooLarge(limit);
                            return HttpCoroutineState::Complete(Err(err));
                        }

                        self.wants_read = true;
                        continue;
                    };

                    self.start += crlf + CRLF.len();
                    self.trailers_len += crlf;

                    if crlf == 0 {
                        self.last_chunk = true;
                    } else {
                        trace!("ignore trailer field of {crlf} bytes");
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rfc9112::chunk::*;

    #[test]
    fn single_chunk() {
        let mut coroutine = Http11ChunksRead::default();
        let out = expect_complete_ok(&mut coroutine, Some(b"5\r\nhello\r\n0\r\n\r\n"));
        assert_eq!(out.body, b"hello");
        assert_eq!(out.remaining, b"");
    }

    #[test]
    fn empty_body() {
        let mut coroutine = Http11ChunksRead::default();
        let out = expect_complete_ok(&mut coroutine, Some(b"0\r\n\r\n"));
        assert!(out.body.is_empty());
    }

    #[test]
    fn ignored_extension() {
        let mut coroutine = Http11ChunksRead::default();
        let out = expect_complete_ok(&mut coroutine, Some(b"5;ext\r\nHello\r\n0\r\n\r\n"));
        assert_eq!(out.body, b"Hello");
    }

    #[test]
    fn invalid_chunk_size() {
        let mut coroutine = Http11ChunksRead::default();
        let err = expect_complete_err(&mut coroutine, Some(b":\r\n0\r\n\r\n"));
        let Http11ChunksReadError::InvalidChunkSize(s) = err else {
            panic!("expected InvalidChunkSize, got {err:?}");
        };
        assert_eq!(s, ":");
    }

    #[test]
    fn signed_chunk_size_errors() {
        let mut coroutine = Http11ChunksRead::default();
        let err = expect_complete_err(&mut coroutine, Some(b"+5\r\nhello\r\n0\r\n\r\n"));
        assert!(matches!(err, Http11ChunksReadError::InvalidChunkSize(_)));
    }

    #[test]
    fn max_usize_chunk_size_errors() {
        let mut coroutine = Http11ChunksRead {
            max_body_len: usize::MAX,
            ..Http11ChunksRead::default()
        };
        let err = expect_complete_err(&mut coroutine, Some(b"FFFFFFFFFFFFFFFF\r\n"));
        assert!(matches!(err, Http11ChunksReadError::BodyTooLarge(_)));
    }

    #[test]
    fn chunk_over_body_limit_errors() {
        let mut coroutine = Http11ChunksRead {
            max_body_len: 8,
            ..Http11ChunksRead::default()
        };
        let err = expect_complete_err(&mut coroutine, Some(b"5\r\nhello\r\n4\r\n"));
        assert!(matches!(err, Http11ChunksReadError::BodyTooLarge(8)));
    }

    #[test]
    fn chunk_line_over_limit_errors() {
        let mut coroutine = Http11ChunksRead::default();
        let line = [b'0'; HTTP11_MAX_CHUNK_LINE_LEN + 2];
        let err = expect_complete_err(&mut coroutine, Some(&line));
        assert!(matches!(err, Http11ChunksReadError::ChunkLineTooLarge(_)));
    }

    #[test]
    fn incomplete_chunk_size_then_resume() {
        let mut coroutine = Http11ChunksRead::default();
        expect_wants_read(&mut coroutine, Some(b"5\r"));
        let out = expect_complete_ok(&mut coroutine, Some(b"\nHello\r\n0\r\n\r\n"));
        assert_eq!(out.body, b"Hello");
    }

    #[test]
    fn incomplete_chunk_data_then_resume() {
        let mut coroutine = Http11ChunksRead::default();
        expect_wants_read(&mut coroutine, Some(b"5\r\nHell"));
        let out = expect_complete_ok(&mut coroutine, Some(b"o\r\n0\r\n\r\n"));
        assert_eq!(out.body, b"Hello");
    }

    #[test]
    fn wiki_ru_multi_chunk() {
        let encoded = "9\r\nchunk 1, \r\n7\r\nchunk 2\r\n0\r\n\r\n";
        let mut coroutine = Http11ChunksRead::default();
        let out = expect_complete_ok(&mut coroutine, Some(encoded.as_bytes()));
        assert_eq!(out.body, b"chunk 1, chunk 2");
    }

    #[test]
    fn github_frewsxcv_test_vector() {
        let encoded = "3\r\nhel\r\nb\r\nlo world!!!\r\n0\r\n\r\n";
        let mut coroutine = Http11ChunksRead::default();
        let out = expect_complete_ok(&mut coroutine, Some(encoded.as_bytes()));
        assert_eq!(out.body, b"hello world!!!");
    }

    fn expect_wants_read(cor: &mut Http11ChunksRead, arg: Option<&[u8]>) {
        match cor.resume(arg) {
            HttpCoroutineState::Yielded(HttpYield::WantsRead) => {}
            state => panic!("expected WantsRead, got {state:?}"),
        }
    }

    fn expect_complete_ok(
        cor: &mut Http11ChunksRead,
        arg: Option<&[u8]>,
    ) -> Http11ChunksReadOutput {
        match cor.resume(arg) {
            HttpCoroutineState::Complete(Ok(out)) => out,
            state => panic!("expected Complete(Ok), got {state:?}"),
        }
    }

    fn expect_complete_err(
        cor: &mut Http11ChunksRead,
        arg: Option<&[u8]>,
    ) -> Http11ChunksReadError {
        match cor.resume(arg) {
            HttpCoroutineState::Complete(Err(err)) => err,
            state => panic!("expected Complete(Err), got {state:?}"),
        }
    }
}
