//! Uniform Resource Identifiers ([RFC 3986]).
//!
//! Builds the URL of a resource from path segments, so that an
//! identifier taken from a caller or a server never changes which
//! resource the request addresses.
//!
//! [RFC 3986]: https://www.rfc-editor.org/rfc/rfc3986

pub mod path;
