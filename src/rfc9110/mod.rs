//! HTTP semantics (RFC 9110).
//!
//! Version-agnostic types shared by all HTTP wire-format modules.
//! RFC 9110 defines methods, status codes, header field semantics,
//! and the abstract request/response message structure that HTTP/1.0,
//! HTTP/1.1, HTTP/2, and HTTP/3 all implement.

pub mod headers;
pub mod request;
pub mod response;
pub mod status;
