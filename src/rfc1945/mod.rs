//! HTTP/1.0 message syntax (RFC 1945).
//!
//! This module implements the HTTP/1.0 wire protocol as I/O-free
//! coroutines. No sockets, no async runtime, and no `std` are
//! required.
//!
//! Shared types (status codes, headers, request, response) live in
//! [`crate::rfc9110`].
//!
//! Key differences from HTTP/1.1 ([`crate::rfc9112`]):
//! - No chunked transfer encoding (`Transfer-Encoding` is not defined)
//! - Connections close after each request by default
//! - `Host` header is not mandatory

pub mod send;
pub mod version;
