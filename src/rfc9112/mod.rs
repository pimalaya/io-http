//! HTTP/1.1 message syntax (RFC 9112).
//!
//! This module implements the HTTP/1.1 wire protocol as I/O-free
//! coroutines. No sockets, no async runtime, and no `std` are
//! required.
//!
//! Shared types (status codes, headers, request, response) live in
//! [`crate::rfc9110`].

pub mod chunk;
pub mod send;
pub mod version;
