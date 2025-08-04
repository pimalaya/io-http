//! I/O-free coroutine to read HTTP response following the Chunked
//! Transfer Coding.
//!
//! Refs: https://datatracker.ietf.org/doc/html/rfc2616#section-3.6.1

use std::mem;

use io_stream::{
    coroutines::read::{ReadStream, ReadStreamError, ReadStreamResult},
    io::StreamIo,
};
use memchr::memmem;
use thiserror::Error;

const CR: u8 = b'\r';
const LF: u8 = b'\n';
const CRLF: [u8; 2] = [CR, LF];
const CRLF_CRLF: [u8; 4] = [CR, LF, CR, LF];

/// Errors that can occur during the coroutine progression.
#[derive(Debug, Error)]
pub enum ReadStreamChunksError {
    /// The coroutine unexpectedly reached the End Of File.
    #[error("Received unexpected EOF")]
    UnexpectedEof,
    /// The coroutine could not exactly read n bytes.
    #[error("Received invalid chunk size: {0}")]
    InvalidChunkSize(String),

    #[error(transparent)]
    ReadStream(#[from] ReadStreamError),
}

/// Send result returned by the coroutine's resume function.
#[derive(Debug)]
pub enum ReadStreamChunksResult {
    /// The coroutine wants stream I/O.
    Io(StreamIo),

    /// The coroutine encountered an error.
    Err(ReadStreamChunksError),

    /// The coroutine has successfully terminated its execution.
    Ok(Vec<u8>),
}

#[derive(Debug)]
enum State {
    ChunkSize,
    // TODO: use ReadStreamExact from io-stream
    ChunkData(usize),
    Trailer,
}

/// I/O-free coroutine to read HTTP response following the Chunked
/// Transfer Coding.
#[derive(Debug)]
pub struct ReadStreamChunks {
    read: ReadStream,
    state: State,
    buffer: Vec<u8>,
    body: Vec<u8>,
}

impl ReadStreamChunks {
    /// Creates a new coroutine from the given [`ReadStream`]
    /// sub-coroutine.
    pub fn new(read: impl Into<ReadStream>) -> Self {
        Self {
            read: read.into(),
            state: State::ChunkSize,
            buffer: Vec::new(),
            body: Vec::new(),
        }
    }

    /// Extends the inner read buffer with the given bytes.
    pub fn extend(&mut self, bytes: impl IntoIterator<Item = u8>) {
        self.buffer.extend(bytes);
    }

    /// Makes the coroutine progress.
    pub fn resume(&mut self, mut arg: Option<StreamIo>) -> ReadStreamChunksResult {
        loop {
            match &mut self.state {
                State::ChunkSize => {
                    // chunk = chunk-size [ chunk-extension ] CRLF
                    //         chunk-data CRLF

                    // find chunk CRLF, otherwise read bytes
                    let Some(crlf) = memmem::find(&self.buffer, &CRLF) else {
                        let output = match self.read.resume(arg.take()) {
                            ReadStreamResult::Ok(output) => output,
                            ReadStreamResult::Err(err) => {
                                return ReadStreamChunksResult::Err(err.into())
                            }
                            ReadStreamResult::Io(io) => return ReadStreamChunksResult::Io(io),
                            ReadStreamResult::Eof => {
                                return ReadStreamChunksResult::Err(
                                    ReadStreamChunksError::UnexpectedEof,
                                )
                            }
                        };
                        self.buffer.extend(output.bytes());
                        self.read.replace(output.buffer);
                        continue;
                    };

                    // search for potential chunk extension
                    let ext = memchr::memchr(b';', &self.buffer[..crlf]).unwrap_or(crlf);

                    // extract chunk size
                    let chunk_size = String::from_utf8_lossy(&self.buffer[..ext]);
                    let Ok(chunk_size) = usize::from_str_radix(&chunk_size, 16) else {
                        let chunk_size = chunk_size.to_string();
                        return ReadStreamChunksResult::Err(
                            ReadStreamChunksError::InvalidChunkSize(chunk_size),
                        );
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

                    // search for chunk data, including the CRLF
                    self.state = State::ChunkData(chunk_size + CRLF.len());
                }
                State::ChunkData(0) => {
                    // no more data to extract, remove last CRLF from
                    // the extracted data then search back for chunk
                    // size
                    self.body.drain(self.body.len() - CRLF.len()..);
                    self.state = State::ChunkSize;
                }
                State::ChunkData(_) if self.buffer.is_empty() => {
                    // empty buffer, read bytes
                    let output = match self.read.resume(arg.take()) {
                        ReadStreamResult::Ok(output) => output,
                        ReadStreamResult::Err(err) => {
                            return ReadStreamChunksResult::Err(err.into())
                        }
                        ReadStreamResult::Io(io) => return ReadStreamChunksResult::Io(io),
                        ReadStreamResult::Eof => {
                            return ReadStreamChunksResult::Err(
                                ReadStreamChunksError::UnexpectedEof,
                            )
                        }
                    };
                    self.buffer.extend(output.bytes());
                    self.read.replace(output.buffer);
                }
                State::ChunkData(size) => {
                    // extract data from buffer, decrease chunk size
                    let min_size = self.buffer.len().min(*size);
                    self.body.extend(self.buffer.drain(..min_size));
                    *size -= min_size;
                }
                State::Trailer => {
                    // a double CRLF CRLF means the end of trailer
                    let Some(0) = memmem::rfind(&self.buffer, &CRLF_CRLF) else {
                        let output = match self.read.resume(arg.take()) {
                            ReadStreamResult::Ok(output) => output,
                            ReadStreamResult::Err(err) => {
                                return ReadStreamChunksResult::Err(err.into())
                            }
                            ReadStreamResult::Io(io) => return ReadStreamChunksResult::Io(io),
                            ReadStreamResult::Eof => {
                                return ReadStreamChunksResult::Err(
                                    ReadStreamChunksError::UnexpectedEof,
                                )
                            }
                        };
                        self.buffer.extend(output.bytes());
                        self.read.replace(output.buffer);
                        continue;
                    };

                    break ReadStreamChunksResult::Ok(mem::take(&mut self.body));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufReader, Read as _};

    use io_stream::{
        coroutines::read::ReadStream,
        io::{StreamIo, StreamOutput},
    };

    use crate::v1_1::coroutines::read_chunks::ReadStreamChunksResult;

    use super::ReadStreamChunks;

    fn test(encoded: &str, decoded: &str) {
        let mut reader = BufReader::new(encoded.as_bytes());

        let read = ReadStream::default();
        let mut http = ReadStreamChunks::new(read);
        let mut arg = None;

        let body = loop {
            match http.resume(arg.take()) {
                ReadStreamChunksResult::Ok(output) => break output,
                ReadStreamChunksResult::Io(StreamIo::Read(Err(mut buffer))) => {
                    let bytes_count = reader.read(&mut buffer).unwrap();
                    let output = StreamOutput {
                        buffer,
                        bytes_count,
                    };
                    arg = Some(StreamIo::Read(Ok(output)))
                }
                other => unreachable!("Unexpected result: {other:?}"),
            }
        };

        assert_eq!(body, decoded.as_bytes());
    }

    /// Test case from russian Wikipedia page:
    ///
    /// https://ru.wikipedia.org/wiki/Chunked_transfer_encoding
    #[test]
    fn wiki_ru() {
        test(
            concat!(
                "9\r\n",
                "chunk 1, \r\n",
                "7\r\n",
                "chunk 2\r\n",
                "0\r\n",
                "\r\n",
            ),
            "chunk 1, chunk 2",
        );
    }

    /// Test case from french Wikipedia page:
    ///
    /// https://fr.wikipedia.org/wiki/Chunked_transfer_encoding
    #[test]
    fn wiki_fr() {
        test(
            concat!(
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
            ),
            concat!(
                "Voici les données du premier morceau\r\n",
                "et voici un second morceau\r\n",
                "et voici deux derniers morceaux ",
                "sans saut de ligne",
            ),
        );
    }

    /// Test case from GitHub repository frewsxcv/rust-chunked-transfer:
    ///
    /// https://github.com/frewsxcv/rust-chunked-transfer/blob/main/src/decoder.rs
    #[test]
    fn github_frewsxcv() {
        test(
            "3\r\nhel\r\nb\r\nlo world!!!\r\n0\r\n\r\n",
            "hello world!!!",
        );
    }
}
