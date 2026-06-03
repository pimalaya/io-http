//! Common HTTP header name constants ([RFC 9110 §5]), lowercase for
//! case-insensitive comparison.
//!
//! [RFC 9110 §5]: https://www.rfc-editor.org/rfc/rfc9110#section-5

/// Header names whose values are redacted in [`core::fmt::Debug`]
/// output to prevent accidental credential leakage in logs.
pub const SENSITIVE_HEADERS: &[&str] = &[
    AUTHORIZATION,
    PROXY_AUTHORIZATION,
    COOKIE,
    SET_COOKIE,
    WWW_AUTHENTICATE,
    PROXY_AUTHENTICATE,
];

/// `Authorization` request header name.
pub const AUTHORIZATION: &str = "authorization";
/// `Connection` general header name.
pub const CONNECTION: &str = "connection";
/// `Content-Length` representation header name.
pub const CONTENT_LENGTH: &str = "content-length";
/// `Cookie` request header name.
pub const COOKIE: &str = "cookie";
/// `Location` response header name.
pub const LOCATION: &str = "location";
/// `Proxy-Authenticate` response header name.
pub const PROXY_AUTHENTICATE: &str = "proxy-authenticate";
/// `Proxy-Authorization` request header name.
pub const PROXY_AUTHORIZATION: &str = "proxy-authorization";
/// `Set-Cookie` response header name.
pub const SET_COOKIE: &str = "set-cookie";
/// `Transfer-Encoding` general header name.
pub const TRANSFER_ENCODING: &str = "transfer-encoding";
/// `WWW-Authenticate` response header name.
pub const WWW_AUTHENTICATE: &str = "www-authenticate";
