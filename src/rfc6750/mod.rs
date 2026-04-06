//! OAuth 2.0 Bearer token usage (RFC 6750).
//!
//! Bearer tokens are opaque strings issued by an authorization server.
//! They are transmitted in the `Authorization` request header:
//!
//! ```text
//! Authorization: Bearer mF_9.B5f-4.1JqM
//! ```
//!
//! Used by JMAP (RFC 8620) and OAuth 2.0 flows to authenticate API
//! requests. The token value itself is obtained separately, for example
//! via the [`io-oauth`] crate.
//!
//! [`io-oauth`]: https://github.com/pimalaya/io-oauth

pub mod bearer;
