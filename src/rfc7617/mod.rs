//! HTTP Basic authentication scheme (RFC 7617).
//!
//! The `Basic` scheme encodes a `username:password` pair as Base64 and
//! transmits it in the `Authorization` request header:
//!
//! ```text
//! Authorization: Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==
//! ```
//!
//! It is used by CardDAV and CalDAV servers that do not support OAuth.
//! Because credentials are only Base64-encoded (not encrypted), the
//! scheme **must** be used over TLS (HTTPS) in production.

pub mod basic;
