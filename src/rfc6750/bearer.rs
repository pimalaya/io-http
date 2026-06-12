//! OAuth 2.0 Bearer token usage: tokens are transmitted as-is in the
//! `Authorization` request header ([RFC 6750 §2.1]).
//!
//! # Example
//!
//! ```rust
//! use io_http::rfc6750::bearer::HttpAuthBearer;
//! use secrecy::ExposeSecret;
//!
//! let token = HttpAuthBearer::new("mF_9.B5f-4.1JqM");
//! assert_eq!(token.to_authorization(), "Bearer mF_9.B5f-4.1JqM");
//!
//! let parsed = HttpAuthBearer::from_authorization("Bearer mF_9.B5f-4.1JqM").unwrap();
//! assert_eq!(parsed.expose_secret(), "mF_9.B5f-4.1JqM");
//! ```
//!
//! [RFC 6750 §2.1]: https://www.rfc-editor.org/rfc/rfc6750#section-2.1

use core::fmt;

use alloc::{format, string::String};

use secrecy::{ExposeSecret, SecretString};
use thiserror::Error;

/// Failure causes when parsing a `Bearer` authorization value.
#[derive(Debug, Error)]
pub enum HttpAuthBearerError {
    #[error("Missing `Bearer ` prefix in Authorization value")]
    MissingPrefix,
}

/// OAuth 2.0 Bearer token; redacted in [`fmt::Debug`], zeroed on drop.
#[derive(Clone)]
pub struct HttpAuthBearer(SecretString);

impl HttpAuthBearer {
    /// Wraps a token string.
    pub fn new(token: impl Into<String>) -> Self {
        Self(SecretString::from(token.into()))
    }

    /// Returns the `Bearer <token>` header value.
    pub fn to_authorization(&self) -> String {
        format!("Bearer {}", self.0.expose_secret())
    }

    /// Parses a `Bearer <token>` header value.
    pub fn from_authorization(value: &str) -> Result<Self, HttpAuthBearerError> {
        value
            .strip_prefix("Bearer ")
            .ok_or(HttpAuthBearerError::MissingPrefix)
            .map(|token| Self(SecretString::from(String::from(token))))
    }
}

impl ExposeSecret<str> for HttpAuthBearer {
    fn expose_secret(&self) -> &str {
        self.0.expose_secret()
    }
}

impl fmt::Debug for HttpAuthBearer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("HttpAuthBearer")
            .field(&"[REDACTED]")
            .finish()
    }
}

impl fmt::Display for HttpAuthBearer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.expose_secret())
    }
}

impl PartialEq for HttpAuthBearer {
    fn eq(&self, other: &Self) -> bool {
        self.0.expose_secret() == other.0.expose_secret()
    }
}

impl Eq for HttpAuthBearer {}

#[cfg(test)]
mod tests {
    use alloc::{format, string::ToString};

    use secrecy::ExposeSecret;

    use crate::rfc6750::bearer::*;

    #[test]
    fn to_authorization_rfc_example() {
        let token = HttpAuthBearer::new("mF_9.B5f-4.1JqM");
        assert_eq!(token.to_authorization(), "Bearer mF_9.B5f-4.1JqM");
    }

    #[test]
    fn to_authorization_has_bearer_prefix() {
        let token = HttpAuthBearer::new("sometoken");
        assert!(token.to_authorization().starts_with("Bearer "));
    }

    #[test]
    fn from_authorization_roundtrip() {
        let original = HttpAuthBearer::new("eyJhbGciOiJSUzI1NiJ9.example");
        let header = original.to_authorization();
        let parsed = HttpAuthBearer::from_authorization(&header).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn from_authorization_missing_prefix() {
        assert!(matches!(
            HttpAuthBearer::from_authorization("Basic dXNlcjpwYXNz"),
            Err(HttpAuthBearerError::MissingPrefix)
        ));
    }

    #[test]
    fn from_authorization_jwt_shaped_token() {
        let value = "Bearer header.payload.signature";
        let token = HttpAuthBearer::from_authorization(value).unwrap();
        assert_eq!(token.expose_secret(), "header.payload.signature");
    }

    #[test]
    fn display_yields_token_string() {
        let token = HttpAuthBearer::new("abc123");
        assert_eq!(token.to_string(), "abc123");
    }

    #[test]
    fn debug_redacts_token() {
        let token = HttpAuthBearer::new("super-secret-token");
        let debug = format!("{token:?}");
        assert!(
            !debug.contains("super-secret-token"),
            "token must not appear in debug"
        );
        assert!(debug.contains("[REDACTED]"));
    }
}
