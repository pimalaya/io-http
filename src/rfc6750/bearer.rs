//! OAuth 2.0 Bearer token usage.
//!
//! Bearer tokens are transmitted as-is in the `Authorization` request
//! header (RFC 6750 §2.1).

use alloc::{format, string::String};
use core::fmt;

use secrecy::{ExposeSecret, SecretString};
use thiserror::Error;

/// Errors that can occur when parsing a `Bearer` authorization value.
#[derive(Debug, Error)]
pub enum BearerError {
    #[error("missing 'Bearer ' prefix in Authorization value")]
    MissingPrefix,
}

/// An OAuth 2.0 Bearer token.
///
/// The token value is stored as a [`SecretString`]: it is redacted in
/// [`fmt::Debug`] output and zeroed in memory on drop.  Use
/// [`ExposeSecret::expose_secret`] to access the raw token string.
///
/// # Example
///
/// ```rust
/// use io_http::rfc6750::bearer::BearerToken;
/// use secrecy::ExposeSecret;
///
/// let token = BearerToken::new("mF_9.B5f-4.1JqM");
/// assert_eq!(token.to_authorization(), "Bearer mF_9.B5f-4.1JqM");
///
/// let parsed = BearerToken::from_authorization("Bearer mF_9.B5f-4.1JqM").unwrap();
/// assert_eq!(parsed.expose_secret(), "mF_9.B5f-4.1JqM");
/// ```
#[derive(Clone)]
pub struct BearerToken(SecretString);

impl BearerToken {
    /// Creates a new bearer token.
    pub fn new(token: impl Into<String>) -> Self {
        Self(SecretString::from(token.into()))
    }

    /// Returns the `Authorization` header value: `Bearer <token>`.
    pub fn to_authorization(&self) -> String {
        format!("Bearer {}", self.0.expose_secret())
    }

    /// Parses an `Authorization` header value of the form `Bearer
    /// <token>`.
    ///
    /// Returns an error if the `Bearer ` prefix is absent.
    pub fn from_authorization(value: &str) -> Result<Self, BearerError> {
        value
            .strip_prefix("Bearer ")
            .ok_or(BearerError::MissingPrefix)
            .map(|token| Self(SecretString::from(String::from(token))))
    }
}

impl ExposeSecret<str> for BearerToken {
    fn expose_secret(&self) -> &str {
        self.0.expose_secret()
    }
}

impl fmt::Debug for BearerToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("BearerToken").field(&"[REDACTED]").finish()
    }
}

impl fmt::Display for BearerToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.expose_secret())
    }
}

impl PartialEq for BearerToken {
    fn eq(&self, other: &Self) -> bool {
        self.0.expose_secret() == other.0.expose_secret()
    }
}

impl Eq for BearerToken {}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;

    use secrecy::ExposeSecret;

    use super::*;

    #[test]
    fn to_authorization_rfc_example() {
        let token = BearerToken::new("mF_9.B5f-4.1JqM");
        assert_eq!(token.to_authorization(), "Bearer mF_9.B5f-4.1JqM");
    }

    #[test]
    fn to_authorization_has_bearer_prefix() {
        let token = BearerToken::new("sometoken");
        assert!(token.to_authorization().starts_with("Bearer "));
    }

    #[test]
    fn from_authorization_roundtrip() {
        let original = BearerToken::new("eyJhbGciOiJSUzI1NiJ9.example");
        let header = original.to_authorization();
        let parsed = BearerToken::from_authorization(&header).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn from_authorization_missing_prefix() {
        assert!(matches!(
            BearerToken::from_authorization("Basic dXNlcjpwYXNz"),
            Err(BearerError::MissingPrefix)
        ));
    }

    #[test]
    fn from_authorization_jwt_shaped_token() {
        let value = "Bearer header.payload.signature";
        let token = BearerToken::from_authorization(value).unwrap();
        assert_eq!(token.expose_secret(), "header.payload.signature");
    }

    #[test]
    fn display_yields_token_string() {
        let token = BearerToken::new("abc123");
        assert_eq!(token.to_string(), "abc123");
    }

    #[test]
    fn debug_redacts_token() {
        let token = BearerToken::new("super-secret-token");
        let debug = alloc::format!("{token:?}");
        assert!(
            !debug.contains("super-secret-token"),
            "token must not appear in debug"
        );
        assert!(debug.contains("[REDACTED]"));
    }
}
