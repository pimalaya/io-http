//! HTTP/1.0 message syntax ([RFC 1945]).
//!
//! I/O-free coroutines implementing the HTTP/1.0 wire protocol: no sockets, no
//! async runtime, no `std`. Shared types live in [`crate::rfc9110`].
//!
//! Differences from HTTP/1.1 ([`crate::rfc9112`]):
//!
//! - No chunked transfer encoding (`Transfer-Encoding` is undefined).
//! - Connections close after each request by default.
//! - `Host` header is not mandatory.
//!
//! [RFC 1945]: https://www.rfc-editor.org/rfc/rfc1945

pub mod request;
pub mod send;
pub mod version;
