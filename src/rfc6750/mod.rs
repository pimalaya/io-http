//! OAuth 2.0 Bearer token usage ([RFC 6750]).
//!
//! Opaque tokens issued by an authorization server, transmitted in the
//! `Authorization` request header (`Bearer <token>`). Used by JMAP (RFC 8620)
//! and OAuth 2.0 flows; the token itself is obtained separately (for example
//! via [`io-oauth`]).
//!
//! [RFC 6750]: https://www.rfc-editor.org/rfc/rfc6750
//! [`io-oauth`]: https://github.com/pimalaya/io-oauth

pub mod bearer;
