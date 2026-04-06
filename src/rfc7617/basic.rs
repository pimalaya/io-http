//! HTTP Basic authentication scheme.
//!
//! The Basic scheme transmits credentials as a base64-encoded
//! `username:password` pair in the `Authorization` request header
//! (RFC 7617 §2).

use alloc::{
    format,
    string::{String, ToString},
};
use core::{fmt, str::from_utf8};

use base64::{DecodeError, prelude::BASE64_STANDARD, prelude::Engine as _};
use secrecy::{ExposeSecret, SecretString};
use thiserror::Error;

/// Errors that can occur when parsing a `Basic` authorization value.
#[derive(Debug, Error)]
pub enum BasicError {
    #[error("Missing 'Basic ' prefix in Authorization value")]
    MissingPrefix,
    #[error("Invalid base64 in Authorization value: {0}")]
    InvalidBase64(DecodeError),
    #[error("Decoded credentials are not valid UTF-8")]
    InvalidUtf8,
    #[error("Decoded credentials are missing the ':' separator")]
    MissingColon,
}

/// An HTTP `Basic` credential pair (username and password).
///
/// The password is stored as a [`SecretString`]: it is redacted in
/// [`fmt::Debug`] output and zeroed in memory on drop.  Use
/// `credentials.password.expose_secret()` to access the raw password
/// string.
///
/// # Example
///
/// ```rust
/// use io_http::rfc7617::basic::BasicCredentials;
/// use secrecy::ExposeSecret;
///
/// let creds = BasicCredentials::new("Aladdin", "open sesame");
/// assert_eq!(creds.to_authorization(), "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==");
///
/// let parsed = BasicCredentials::from_authorization("Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==").unwrap();
/// assert_eq!(parsed.username, "Aladdin");
/// assert_eq!(parsed.password.expose_secret(), "open sesame");
/// ```
#[derive(Clone)]
pub struct BasicCredentials {
    /// The username.
    pub username: String,
    /// The password.
    ///
    /// Use [`ExposeSecret::expose_secret`] to access the value.
    pub password: SecretString,
}

impl BasicCredentials {
    /// Creates a new credential pair.
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            password: SecretString::from(password.into()),
        }
    }

    /// Returns the `Authorization` header value: `Basic <base64(username:password)>`.
    pub fn to_authorization(&self) -> String {
        let payload = format!("{}:{}", self.username, self.password.expose_secret());
        let encoded = BASE64_STANDARD.encode(payload.as_bytes());
        format!("Basic {encoded}")
    }

    /// Parses an `Authorization` header value of the form `Basic
    /// <b64>`.
    ///
    /// Returns an error if the prefix is missing, the base64 is
    /// invalid, or the decoded string does not contain a `:`
    /// separator.
    pub fn from_authorization(value: &str) -> Result<Self, BasicError> {
        let encoded = value
            .strip_prefix("Basic ")
            .ok_or(BasicError::MissingPrefix)?;

        let decoded = BASE64_STANDARD
            .decode(encoded)
            .map_err(BasicError::InvalidBase64)?;

        let s = from_utf8(&decoded).map_err(|_| BasicError::InvalidUtf8)?;
        let (username, password) = s.split_once(':').ok_or(BasicError::MissingColon)?;

        Ok(Self {
            username: username.into(),
            password: SecretString::from(password.to_string()),
        })
    }
}

impl fmt::Debug for BasicCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BasicCredentials")
            .field("username", &self.username)
            .field("password", &"[REDACTED]")
            .finish()
    }
}

impl PartialEq for BasicCredentials {
    fn eq(&self, other: &Self) -> bool {
        self.username == other.username
            && self.password.expose_secret() == other.password.expose_secret()
    }
}

impl Eq for BasicCredentials {}

#[cfg(test)]
mod tests {
    use secrecy::ExposeSecret;

    use super::*;

    #[test]
    fn to_authorization_rfc_test_vector() {
        let creds = BasicCredentials::new("Aladdin", "open sesame");
        assert_eq!(
            creds.to_authorization(),
            "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ=="
        );
    }

    #[test]
    fn to_authorization_has_basic_prefix() {
        let creds = BasicCredentials::new("user", "pass");
        assert!(creds.to_authorization().starts_with("Basic "));
    }

    #[test]
    fn to_authorization_empty_password() {
        let creds = BasicCredentials::new("user", "");
        let value = creds.to_authorization();
        let decoded = BasicCredentials::from_authorization(&value).unwrap();
        assert_eq!(decoded.username, "user");
        assert_eq!(decoded.password.expose_secret(), "");
    }

    #[test]
    fn from_authorization_roundtrip() {
        let original = BasicCredentials::new("user@example.com", "p@$$w0rd!");
        let header = original.to_authorization();
        let parsed = BasicCredentials::from_authorization(&header).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn from_authorization_colon_in_password() {
        let original = BasicCredentials::new("user", "pa:ss:word");
        let parsed = BasicCredentials::from_authorization(&original.to_authorization()).unwrap();
        assert_eq!(parsed.username, "user");
        assert_eq!(parsed.password.expose_secret(), "pa:ss:word");
    }

    #[test]
    fn from_authorization_missing_prefix() {
        assert!(matches!(
            BasicCredentials::from_authorization("Bearer token"),
            Err(BasicError::MissingPrefix)
        ));
    }

    #[test]
    fn from_authorization_invalid_base64() {
        assert!(matches!(
            BasicCredentials::from_authorization("Basic !!!not-b64!!!"),
            Err(BasicError::InvalidBase64(_))
        ));
    }

    #[test]
    fn from_authorization_missing_colon() {
        // base64("nocolon") = "bm9jb2xvbg=="
        assert!(matches!(
            BasicCredentials::from_authorization("Basic bm9jb2xvbg=="),
            Err(BasicError::MissingColon)
        ));
    }

    #[test]
    fn debug_redacts_password() {
        let creds = BasicCredentials::new("alice", "hunter2");
        let debug = alloc::format!("{creds:?}");
        assert!(
            !debug.contains("hunter2"),
            "password must not appear in debug"
        );
        assert!(debug.contains("[REDACTED]"));
        assert!(debug.contains("alice"), "username must appear in debug");
    }
}
