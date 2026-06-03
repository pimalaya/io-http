//! Common HTTP header name constants ([RFC 9110 §5]), lowercase.
//!
//! [RFC 9110 §5]: https://www.rfc-editor.org/rfc/rfc9110#section-5

/// Header names whose values are redacted in [`core::fmt::Debug`] output.
pub const SENSITIVE_HEADERS: &[&str] = &[
    AUTHORIZATION,
    PROXY_AUTHORIZATION,
    COOKIE,
    SET_COOKIE,
    WWW_AUTHENTICATE,
    PROXY_AUTHENTICATE,
];

pub const AUTHORIZATION: &str = "authorization";
pub const CONNECTION: &str = "connection";
pub const CONTENT_LENGTH: &str = "content-length";
pub const COOKIE: &str = "cookie";
pub const LOCATION: &str = "location";
pub const PROXY_AUTHENTICATE: &str = "proxy-authenticate";
pub const PROXY_AUTHORIZATION: &str = "proxy-authorization";
pub const SET_COOKIE: &str = "set-cookie";
pub const TRANSFER_ENCODING: &str = "transfer-encoding";
pub const WWW_AUTHENTICATE: &str = "www-authenticate";
