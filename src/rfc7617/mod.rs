//! HTTP Basic authentication scheme ([RFC 7617]).
//!
//! Base64 `username:password` pair carried in the `Authorization` request
//! header (`Basic <base64(user:pass)>`). Used by CardDAV/CalDAV servers that do
//! not support OAuth. Credentials are only encoded, not encrypted: the scheme
//! must be used over TLS in production.
//!
//! [RFC 7617]: https://www.rfc-editor.org/rfc/rfc7617

pub mod basic;
