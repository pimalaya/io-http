//! HTTP semantics ([RFC 9110]).
//!
//! Version-agnostic types shared by every HTTP wire-format module: methods,
//! status codes, header field semantics, and the abstract request/response
//! message structure that HTTP/1.0, HTTP/1.1, HTTP/2, and HTTP/3 all implement.
//!
//! [RFC 9110]: https://www.rfc-editor.org/rfc/rfc9110

pub mod challenge;
pub mod chars;
pub mod headers;
pub mod request;
pub mod response;
pub mod send;
pub mod status;
