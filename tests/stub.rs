//! Stub I/O stream for driving coroutines against pre-crafted
//! response buffers.
//!
//! [`StubStream`] implements [`Read`] + [`Write`] on top of an
//! in-memory cursor. Pair it with [`handle`] (re-exported from
//! [`io_socket::runtimes::std_stream`]) to drive any coroutine
//! without a network connection.
//!
//! Reads drain bytes from the response buffer provided at
//! construction; writes are silently discarded (the serialized
//! request is not asserted).

use std::io::{Cursor, Read, Result, Write};

/// An in-memory stream backed by a pre-crafted response buffer.
pub struct StubStream<'a> {
    response: Cursor<&'a [u8]>,
}

impl<'a> StubStream<'a> {
    pub fn new(response: &'a [u8]) -> Self {
        Self {
            response: Cursor::new(response),
        }
    }
}

impl Read for StubStream<'_> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.response.read(buf)
    }
}

impl Write for StubStream<'_> {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        Ok(buf.len())
    }

    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}
