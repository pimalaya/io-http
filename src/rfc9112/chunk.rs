//! I/O-free coroutine decoding a `Transfer-Encoding: chunked` body
//! ([RFC 9112 §7.1]) into a single buffer. For incremental consumption
//! use [`super::chunk_stream::Http11ReadChunksStream`].
//!
//! [RFC 9112 §7.1]: https://www.rfc-editor.org/rfc/rfc9112#section-7.1

use core::mem;

use alloc::{
    string::{String, ToString},
    vec::Vec,
};

use log::trace;
use memchr::{memchr, memmem};
use thiserror::Error;

use crate::{coroutine::*, rfc9110::chars::CRLF};

/// Failure causes during the HTTP/1.1 chunked-body read flow.
#[derive(Debug, Error)]
pub enum Http11ReadChunksError {
    #[error("HTTP/1.1 read chunks failed: invalid chunk size `{0}`")]
    InvalidChunkSize(String),
}

/// Terminal output of [`Http11ReadChunks`].
#[derive(Debug)]
pub struct Http11ReadChunksOutput {
    pub body: Vec<u8>,
    pub remaining: Vec<u8>,
}

#[derive(Debug, Default)]
enum State {
    #[default]
    ChunkSize,
    ChunkData(usize),
}

/// I/O-free coroutine to read an HTTP response body using chunked
/// transfer coding.
#[derive(Debug, Default)]
pub struct Http11ReadChunks {
    state: State,
    wants_read: bool,
    last_chunk: bool,
    buf: Vec<u8>,
    body: Vec<u8>,
}

impl HttpCoroutine for Http11ReadChunks {
    type Yield = HttpYield;
    type Return = Result<Http11ReadChunksOutput, Http11ReadChunksError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> HttpCoroutineState<Self::Yield, Self::Return> {
        if let Some(data) = arg {
            self.buf.extend_from_slice(data);
        }

        loop {
            if self.wants_read {
                self.wants_read = false;
                return HttpCoroutineState::Yielded(HttpYield::WantsRead);
            }

            if self.last_chunk {
                let body = mem::take(&mut self.body);
                let remaining = mem::take(&mut self.buf);
                return HttpCoroutineState::Complete(Ok(Http11ReadChunksOutput {
                    body,
                    remaining,
                }));
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
                        let err = Http11ReadChunksError::InvalidChunkSize(chunk_size);
                        return HttpCoroutineState::Complete(Err(err));
                    };

                    self.buf.drain(..crlf + CRLF.len());
                    trace!("reading chunk of {n} bytes");
                    self.state = State::ChunkData(n);
                }
                State::ChunkData(size) if self.buf.len() < size + CRLF.len() => {
                    trace!("received incomplete chunk data {}/{size}", self.buf.len());
                    self.wants_read = true;
                    continue;
                }
                State::ChunkData(size) => {
                    self.body.extend(self.buf.drain(..size));
                    self.buf.drain(..CRLF.len());
                    self.state = State::ChunkSize;
                    self.last_chunk = size == 0;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_chunk() {
        let mut coroutine = Http11ReadChunks::default();
        let out = expect_complete_ok(&mut coroutine, Some(b"5\r\nhello\r\n0\r\n\r\n"));
        assert_eq!(out.body, b"hello");
        assert_eq!(out.remaining, b"");
    }

    #[test]
    fn empty_body() {
        let mut coroutine = Http11ReadChunks::default();
        let out = expect_complete_ok(&mut coroutine, Some(b"0\r\n\r\n"));
        assert!(out.body.is_empty());
    }

    #[test]
    fn ignored_extension() {
        let mut coroutine = Http11ReadChunks::default();
        let out = expect_complete_ok(&mut coroutine, Some(b"5;ext\r\nHello\r\n0\r\n\r\n"));
        assert_eq!(out.body, b"Hello");
    }

    #[test]
    fn invalid_chunk_size() {
        let mut coroutine = Http11ReadChunks::default();
        let err = expect_complete_err(&mut coroutine, Some(b":\r\n0\r\n\r\n"));
        let Http11ReadChunksError::InvalidChunkSize(s) = err;
        assert_eq!(s, ":");
    }

    #[test]
    fn incomplete_chunk_size_then_resume() {
        let mut coroutine = Http11ReadChunks::default();
        expect_wants_read(&mut coroutine, Some(b"5\r"));
        let out = expect_complete_ok(&mut coroutine, Some(b"\nHello\r\n0\r\n\r\n"));
        assert_eq!(out.body, b"Hello");
    }

    #[test]
    fn incomplete_chunk_data_then_resume() {
        let mut coroutine = Http11ReadChunks::default();
        expect_wants_read(&mut coroutine, Some(b"5\r\nHell"));
        let out = expect_complete_ok(&mut coroutine, Some(b"o\r\n0\r\n\r\n"));
        assert_eq!(out.body, b"Hello");
    }

    #[test]
    fn wiki_ru_multi_chunk() {
        let encoded = "9\r\nchunk 1, \r\n7\r\nchunk 2\r\n0\r\n\r\n";
        let mut coroutine = Http11ReadChunks::default();
        let out = expect_complete_ok(&mut coroutine, Some(encoded.as_bytes()));
        assert_eq!(out.body, b"chunk 1, chunk 2");
    }

    #[test]
    fn github_frewsxcv_test_vector() {
        let encoded = "3\r\nhel\r\nb\r\nlo world!!!\r\n0\r\n\r\n";
        let mut coroutine = Http11ReadChunks::default();
        let out = expect_complete_ok(&mut coroutine, Some(encoded.as_bytes()));
        assert_eq!(out.body, b"hello world!!!");
    }

    // --- utils

    fn expect_wants_read(cor: &mut Http11ReadChunks, arg: Option<&[u8]>) {
        match cor.resume(arg) {
            HttpCoroutineState::Yielded(HttpYield::WantsRead) => {}
            state => panic!("expected WantsRead, got {state:?}"),
        }
    }

    fn expect_complete_ok(
        cor: &mut Http11ReadChunks,
        arg: Option<&[u8]>,
    ) -> Http11ReadChunksOutput {
        match cor.resume(arg) {
            HttpCoroutineState::Complete(Ok(out)) => out,
            state => panic!("expected Complete(Ok), got {state:?}"),
        }
    }

    fn expect_complete_err(
        cor: &mut Http11ReadChunks,
        arg: Option<&[u8]>,
    ) -> Http11ReadChunksError {
        match cor.resume(arg) {
            HttpCoroutineState::Complete(Err(err)) => err,
            state => panic!("expected Complete(Err), got {state:?}"),
        }
    }
}
