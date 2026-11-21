//! HTTP/1.1 message syntax ([RFC 9112]).
//!
//! I/O-free coroutines implementing the HTTP/1.1 wire protocol: no sockets, no
//! async runtime, no `std`. Shared types (status codes, headers, request,
//! response) live in [`crate::rfc9110`].
//!
//! [RFC 9112]: https://www.rfc-editor.org/rfc/rfc9112

#[cfg(feature = "client")]
#[cfg_attr(docsrs, doc(cfg(feature = "client")))]
pub mod chunk;
#[cfg(feature = "client")]
#[cfg_attr(docsrs, doc(cfg(feature = "client")))]
pub mod chunk_stream;
#[cfg(feature = "client")]
#[cfg_attr(docsrs, doc(cfg(feature = "client")))]
pub mod read_headers;
pub mod request;
#[cfg(feature = "client")]
#[cfg_attr(docsrs, doc(cfg(feature = "client")))]
pub mod send;
pub mod version;
