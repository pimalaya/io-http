//! HTTP Basic authentication scheme: credentials are sent as a
//! base64-encoded `username:password` pair in the `Authorization`
//! request header ([RFC 7617 §2]).
//!
//! # Example
//!
//! ```rust
//! use io_http::rfc7617::basic::HttpAuthBasic;
//! use secrecy::ExposeSecret;
//!
//! let creds = HttpAuthBasic::new("Aladdin", "open sesame");
//! assert_eq!(creds.to_authorization(), "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==");
//!
//! let parsed = HttpAuthBasic::from_authorization("Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==").unwrap();
//! assert_eq!(parsed.username, "Aladdin");
//! assert_eq!(parsed.password.expose_secret(), "open sesame");
//! ```
//!
//! [RFC 7617 §2]: https://www.rfc-editor.org/rfc/rfc7617#section-2

use core::{fmt, str::from_utf8};

use alloc::{
    format,
    string::{String, ToString},
};

use base64::{DecodeError, prelude::BASE64_STANDARD, prelude::Engine as _};
use secrecy::{ExposeSecret, SecretString};
use thiserror::Error;

/// Failure causes when parsing a `Basic` authorization value.
#[derive(Debug, Error)]
pub enum HttpAuthBasicError {
    /// The value does not start with the basic scheme prefix.
    #[error("Missing `Basic ` prefix in Authorization value")]
    MissingPrefix,
    /// The credentials payload is not valid base64.
    #[error("Invalid base64 in Authorization value: {0}")]
    InvalidBase64(DecodeError),
    /// The decoded credentials are not valid UTF-8.
    #[error("Decoded credentials are not valid UTF-8")]
    InvalidUtf8,
    /// The decoded credentials carry no colon separator.
    #[error("Decoded credentials are missing the `:` separator")]
    MissingColon,
}

/// HTTP `Basic` credential pair; `password` is redacted in
/// [`fmt::Debug`] and zeroed on drop.
#[derive(Clone)]
pub struct HttpAuthBasic {
    /// The username, in clear.
    pub username: String,
    /// The password, redacted in [`fmt::Debug`] and zeroed on drop.
    pub password: SecretString,
}

impl HttpAuthBasic {
    /// Wraps a username + password.
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            password: SecretString::from(password.into()),
        }
    }

    /// Returns the `Basic <base64(user:pass)>` header value.
    pub fn to_authorization(&self) -> String {
        let payload = format!("{}:{}", self.username, self.password.expose_secret());
        let encoded = BASE64_STANDARD.encode(payload.as_bytes());
        format!("Basic {encoded}")
    }

    /// Parses a `Basic <b64>` header value.
    pub fn from_authorization(value: &str) -> Result<Self, HttpAuthBasicError> {
        let encoded = value
            .strip_prefix("Basic ")
            .ok_or(HttpAuthBasicError::MissingPrefix)?;

        let decoded = BASE64_STANDARD
            .decode(encoded)
            .map_err(HttpAuthBasicError::InvalidBase64)?;

        let s = from_utf8(&decoded).map_err(|_| HttpAuthBasicError::InvalidUtf8)?;
        let (username, password) = s.split_once(':').ok_or(HttpAuthBasicError::MissingColon)?;

        Ok(Self {
            username: username.into(),
            password: SecretString::from(password.to_string()),
        })
    }
}

impl fmt::Debug for HttpAuthBasic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpAuthBasic")
            .field("username", &self.username)
            .field("password", &"[REDACTED]")
            .finish()
    }
}

impl PartialEq for HttpAuthBasic {
    fn eq(&self, other: &Self) -> bool {
        self.username == other.username
            && self.password.expose_secret() == other.password.expose_secret()
    }
}

impl Eq for HttpAuthBasic {}

#[cfg(test)]
mod tests {
    use alloc::format;

    use secrecy::ExposeSecret;

    use crate::rfc7617::basic::*;

    #[test]
    fn to_authorization_rfc_test_vector() {
        let creds = HttpAuthBasic::new("Aladdin", "open sesame");
        assert_eq!(
            creds.to_authorization(),
            "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ=="
        );
    }

    #[test]
    fn to_authorization_has_basic_prefix() {
        let creds = HttpAuthBasic::new("user", "pass");
        assert!(creds.to_authorization().starts_with("Basic "));
    }

    #[test]
    fn to_authorization_empty_password() {
        let creds = HttpAuthBasic::new("user", "");
        let value = creds.to_authorization();
        let decoded = HttpAuthBasic::from_authorization(&value).unwrap();
        assert_eq!(decoded.username, "user");
        assert_eq!(decoded.password.expose_secret(), "");
    }

    #[test]
    fn from_authorization_roundtrip() {
        let original = HttpAuthBasic::new("user@example.com", "p@$$w0rd!");
        let header = original.to_authorization();
        let parsed = HttpAuthBasic::from_authorization(&header).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn from_authorization_colon_in_password() {
        let original = HttpAuthBasic::new("user", "pa:ss:word");
        let parsed = HttpAuthBasic::from_authorization(&original.to_authorization()).unwrap();
        assert_eq!(parsed.username, "user");
        assert_eq!(parsed.password.expose_secret(), "pa:ss:word");
    }

    #[test]
    fn from_authorization_missing_prefix() {
        assert!(matches!(
            HttpAuthBasic::from_authorization("Bearer token"),
            Err(HttpAuthBasicError::MissingPrefix)
        ));
    }

    #[test]
    fn from_authorization_invalid_base64() {
        assert!(matches!(
            HttpAuthBasic::from_authorization("Basic !!!not-b64!!!"),
            Err(HttpAuthBasicError::InvalidBase64(_))
        ));
    }

    #[test]
    fn from_authorization_missing_colon() {
        // NOTE: base64("nocolon") = "bm9jb2xvbg=="
        assert!(matches!(
            HttpAuthBasic::from_authorization("Basic bm9jb2xvbg=="),
            Err(HttpAuthBasicError::MissingColon)
        ));
    }

    #[test]
    fn debug_redacts_password() {
        let creds = HttpAuthBasic::new("alice", "hunter2");
        let debug = format!("{creds:?}");
        assert!(
            !debug.contains("hunter2"),
            "password must not appear in debug"
        );
        assert!(debug.contains("[REDACTED]"));
        assert!(debug.contains("alice"), "username must appear in debug");
    }
}
