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

use core::mem;

use alloc::{
    string::{String, ToString},
    vec::Vec,
};

use log::trace;
use memchr::memmem;
use thiserror::Error;

const CRLF: [u8; 2] = [b'\r', b'\n'];

/// Errors that can occur during the coroutine progression.
#[derive(Debug, Error)]
pub enum Http11ReadChunksError {
    #[error("Invalid chunk size `{0}`")]
    InvalidChunkSize(String),
}

/// Result returned by [`Http11ReadChunks::resume`].
#[derive(Debug)]
pub enum Http11ReadChunksResult {
    /// The coroutine has successfully terminated its execution.
    Ok { body: Vec<u8>, remaining: Vec<u8> },
    /// The coroutine needs a socket I/O to be performed.
    WantsRead,
    /// The coroutine encountered an error.
    Err(Http11ReadChunksError),
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

impl Http11ReadChunks {
    /// Advances the coroutine.
    ///
    /// Pass `None` on the first call. On subsequent calls, pass the
    /// [`SocketOutput`] returned by the runtime after processing the
    /// last emitted [`SocketInput`].
    pub fn resume(&mut self, arg: &[u8]) -> Http11ReadChunksResult {
        self.buf.extend_from_slice(arg);

        loop {
            if self.wants_read {
                self.wants_read = false;
                return Http11ReadChunksResult::WantsRead;
            }

            if self.last_chunk {
                let body = mem::take(&mut self.body);
                let remaining = mem::take(&mut self.buf);
                return Http11ReadChunksResult::Ok { body, remaining };
            }

            match self.state {
                State::ChunkSize => {
                    let Some(crlf) = memmem::find(&self.buf, &CRLF) else {
                        self.wants_read = true;
                        continue;
                    };

                    // search for potential chunk extension
                    let ext = match memchr::memchr(b';', &self.buf[..crlf]) {
                        None => crlf,
                        Some(ext) => {
                            let exts = String::from_utf8_lossy(self.buf[ext..crlf].trim_ascii());
                            trace!("ignore extension(s) `{exts}`");
                            ext
                        }
                    };

                    // extract chunk size
                    let chunk_size = String::from_utf8_lossy(self.buf[..ext].trim_ascii());

                    let Ok(n) = usize::from_str_radix(&chunk_size, 16) else {
                        let chunk_size = chunk_size.to_string();
                        let err = Http11ReadChunksError::InvalidChunkSize(chunk_size);
                        return Http11ReadChunksResult::Err(err);
                    };

                    self.buf.drain(..crlf + CRLF.len());
                    self.state = State::ChunkData(n);
                }
                State::ChunkData(size) if self.buf.len() < size + CRLF.len() => {
                    self.wants_read = true;
                    continue;
                }
                State::ChunkData(size) => {
                    // drain <size> from buffer
                    self.body.extend(self.buf.drain(..size));
                    // remove trailing \r\n from buffer
                    self.buf.drain(..CRLF.len());
                    // look for next chunk size
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

    fn test(encoded: &str, decoded: &str) {
        let mut coroutine = Http11ReadChunks::default();

        loop {
            match coroutine.resume(encoded.as_bytes()) {
                Http11ReadChunksResult::Ok { body, remaining } => {
                    assert_eq!(body, decoded.as_bytes());
                    assert_eq!(remaining, b"");
                    break;
                }
                Http11ReadChunksResult::WantsRead => unreachable!("wants read"),
                Http11ReadChunksResult::Err(err) => unreachable!("{err}"),
            }
        }
    }

    #[test]
    fn incomplete_chunk_size() {
        let mut coroutine = Http11ReadChunks::default();
        let mut buf: &[u8] = b"5\r";

        loop {
            match coroutine.resume(buf) {
                Http11ReadChunksResult::Ok { body, remaining } => {
                    assert_eq!(body, b"Hello");
                    assert_eq!(remaining, b"");
                    break;
                }
                Http11ReadChunksResult::WantsRead => buf = b"\nHello\r\n0\r\n\r\n",
                Http11ReadChunksResult::Err(err) => unreachable!("{err}"),
            }
        }
    }

    #[test]
    fn invalid_chunk_size() {
        let mut coroutine = Http11ReadChunks::default();
        let buf: &[u8] = b":\r\n0\r\n\r\n";

        loop {
            match coroutine.resume(buf) {
                Http11ReadChunksResult::Ok { .. } => unreachable!("Ok"),
                Http11ReadChunksResult::WantsRead => unreachable!("WantsRead"),
                Http11ReadChunksResult::Err(Http11ReadChunksError::InvalidChunkSize(s)) => {
                    break assert_eq!(":", s);
                }
            }
        }
    }

    #[test]
    fn chunk_size_ext() {
        test("5;ext\r\nHello\r\n0\r\n\r\n", "Hello");
    }

    #[test]
    fn incomplete_chunk_data() {
        let mut coroutine = Http11ReadChunks::default();
        let mut buf: &[u8] = b"5\r\nHell";

        loop {
            match coroutine.resume(buf) {
                Http11ReadChunksResult::Ok { body, remaining } => {
                    assert_eq!(body, b"Hello");
                    assert_eq!(remaining, b"");
                    break;
                }
                Http11ReadChunksResult::WantsRead => buf = b"o\r\n0\r\n\r\n",
                Http11ReadChunksResult::Err(err) => unreachable!("{err}"),
            }
        }
    }

    /// Test case from the Russian Wikipedia page on chunked transfer
    /// encoding.
    ///
    /// https://ru.wikipedia.org/wiki/Chunked_transfer_encoding
    #[test]
    fn wiki_ru() {
        let encoded = concat!(
            "9\r\n",
            "chunk 1, \r\n",
            "7\r\n",
            "chunk 2\r\n",
            "0\r\n",
            "\r\n",
        );

        let decoded = "chunk 1, chunk 2";

        test(encoded, decoded);
    }

    /// Test case from the French Wikipedia page on chunked transfer
    /// encoding.
    ///
    /// https://fr.wikipedia.org/wiki/Chunked_transfer_encoding
    #[test]
    fn chunks_wiki_fr() {
        let encoded = concat!(
            "27\r\n",
            "Voici les données du premier morceau\r\n\r\n",
            "1C\r\n",
            "et voici un second morceau\r\n\r\n",
            "20\r\n",
            "et voici deux derniers morceaux \r\n",
            "12\r\n",
            "sans saut de ligne\r\n",
            "0\r\n",
            "\r\n",
        );

        let decoded = concat!(
            "Voici les données du premier morceau\r\n",
            "et voici un second morceau\r\n",
            "et voici deux derniers morceaux ",
            "sans saut de ligne",
        );

        test(encoded, decoded);
    }

    /// Test case from the frewsxcv/rust-chunked-transfer repository.
    ///
    /// https://github.com/frewsxcv/rust-chunked-transfer/blob/main/src/decoder.rs
    #[test]
    fn chunks_github_frewsxcv() {
        let encoded = "3\r\nhel\r\nb\r\nlo world!!!\r\n0\r\n\r\n";
        let decoded = "hello world!!!";
        test(encoded, decoded);
    }

    #[test]
    fn chunks_single() {
        test("5\r\nhello\r\n0\r\n\r\n", "hello");
    }

    #[test]
    fn chunks_empty_body() {
        test("0\r\n\r\n", "");
    }
}
