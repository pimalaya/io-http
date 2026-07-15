//! Common HTTP header name constants ([RFC 9110 §5]), lowercase.
//!
//! [RFC 9110 §5]: https://www.rfc-editor.org/rfc/rfc9110#section-5

/// Header names whose values are redacted in [`core::fmt::Debug`] output.
pub const HTTP_SENSITIVE_HEADERS: &[&str] = &[
    HTTP_AUTHORIZATION,
    HTTP_PROXY_AUTHORIZATION,
    HTTP_COOKIE,
    HTTP_SET_COOKIE,
    HTTP_WWW_AUTHENTICATE,
    HTTP_PROXY_AUTHENTICATE,
];

/// The authorization request header name.
pub const HTTP_AUTHORIZATION: &str = "authorization";
/// The connection header name.
pub const HTTP_CONNECTION: &str = "connection";
/// The content length header name.
pub const HTTP_CONTENT_LENGTH: &str = "content-length";
/// The cookie request header name.
pub const HTTP_COOKIE: &str = "cookie";
/// The host request header name.
pub const HTTP_HOST: &str = "host";
/// The location response header name.
pub const HTTP_LOCATION: &str = "location";
/// The proxy authenticate response header name.
pub const HTTP_PROXY_AUTHENTICATE: &str = "proxy-authenticate";
/// The proxy authorization request header name.
pub const HTTP_PROXY_AUTHORIZATION: &str = "proxy-authorization";
/// The set-cookie response header name.
pub const HTTP_SET_COOKIE: &str = "set-cookie";
/// The transfer encoding header name.
pub const HTTP_TRANSFER_ENCODING: &str = "transfer-encoding";
/// The authenticate response header name.
pub const HTTP_WWW_AUTHENTICATE: &str = "www-authenticate";
