//! https://datatracker.ietf.org/doc/html/rfc2616#section-3.6.1

use std::mem;

use io_stream::{coroutines::Read, Io};
use memchr::memmem;

const CR: u8 = b'\r';
const CRLF: [u8; 2] = [CR, LF];
const LF: u8 = b'\n';

const CRLF_CRLF: [u8; 4] = [CR, LF, CR, LF];

#[derive(Debug)]
pub enum State {
    ChunkSize,
    // TODO: use ReadExact from io-stream
    ChunkData(usize),
    Trailer,
}

#[derive(Debug)]
pub struct ChunkedTransferCoding {
    read: Read,
    state: State,
    buffer: Vec<u8>,
    body: Vec<u8>,
}

impl ChunkedTransferCoding {
    pub fn new(read: impl Into<Read>) -> Self {
        Self {
            read: read.into(),
            state: State::ChunkSize,
            buffer: Vec::new(),
            body: Vec::new(),
        }
    }

    pub fn extend(&mut self, bytes: impl IntoIterator<Item = u8>) {
        self.buffer.extend(bytes);
    }

    pub fn resume(&mut self, mut input: Option<Io>) -> Result<Vec<u8>, Io> {
        loop {
            match &mut self.state {
                State::ChunkSize => {
                    // chunk = chunk-size [ chunk-extension ] CRLF
                    //         chunk-data CRLF

                    // find chunk CRLF, otherwise read bytes
                    let Some(crlf) = memmem::find(&self.buffer, &CRLF) else {
                        let output = self.read.resume(input.take())?;
                        self.buffer.extend(output.bytes());
                        self.read.replace(output.buffer);
                        continue;
                    };

                    // search for potential chunk extension
                    let ext = memchr::memchr(b';', &self.buffer[..crlf]).unwrap_or(crlf);

                    // extract chunk size
                    let chunk_size = String::from_utf8_lossy(&self.buffer[..ext]);
                    let Ok(chunk_size) = usize::from_str_radix(&chunk_size, 16) else {
                        return Err(Io::Error(format!("invalid chunk size: {chunk_size}")));
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
                    let output = self.read.resume(input.take())?;
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
                        let output = self.read.resume(input.take())?;
                        self.buffer.extend(output.bytes());
                        self.read.replace(output.buffer);
                        continue;
                    };

                    break Ok(mem::take(&mut self.body));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufReader, Read as _};

    use io_stream::{coroutines::Read, Io, Output};

    use super::ChunkedTransferCoding;

    fn test(encoded: &str, decoded: &str) {
        let mut reader = BufReader::new(encoded.as_bytes());

        let read = Read::default();
        let mut http = ChunkedTransferCoding::new(read);
        let mut arg = None;

        let body = loop {
            match http.resume(arg.take()) {
                Ok(body) => break body,
                Err(Io::Read(Err(mut buffer))) => {
                    let bytes_count = reader.read(&mut buffer).unwrap();

                    let output = Output {
                        buffer,
                        bytes_count,
                    };

                    arg = Some(Io::Read(Ok(output)))
                }
                Err(io) => unreachable!("unexpected I/O: {io:?}"),
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
