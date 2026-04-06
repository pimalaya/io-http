//! HTTP response type (RFC 9110 §15).

use alloc::{string::String, vec::Vec};
use core::fmt;

use crate::rfc9110::{headers::SENSITIVE_HEADERS, status::StatusCode};

/// An incoming HTTP response.
#[derive(Clone)]
pub struct HttpResponse {
    /// HTTP status code.
    pub status: StatusCode,
    /// HTTP protocol version string (e.g. `"HTTP/1.1"`, `"HTTP/1.0"`).
    pub version: String,
    /// Response headers as `(name, value)` pairs (names stored in lowercase).
    pub headers: Vec<(String, String)>,
    /// Response body bytes.
    pub body: Vec<u8>,
}

impl HttpResponse {
    /// Returns the value of the first header with the given name
    /// (case-insensitive), if any.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Incremental builder for [`HttpResponse`], used internally by
/// wire-format send coroutines.
#[derive(Clone, Debug)]
pub(crate) struct ResponseBuilder {
    pub(crate) status: Option<StatusCode>,
    pub(crate) version: String,
    pub(crate) headers: Vec<(String, String)>,
}

impl Default for ResponseBuilder {
    fn default() -> Self {
        Self {
            status: None,
            version: "HTTP/1.1".into(),
            headers: Vec::new(),
        }
    }
}

impl ResponseBuilder {
    /// Adds a header (name stored in lowercase).
    pub(crate) fn header(&mut self, name: &str, value: &[u8]) {
        let value = String::from_utf8_lossy(value).into_owned();
        self.headers.push((name.to_lowercase(), value));
    }

    /// Returns the value of the first header with the given name
    /// (case-insensitive), if any.
    pub(crate) fn get_header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// Finalizes the builder into an [`HttpResponse`].
    pub(crate) fn build(self, body: Vec<u8>) -> HttpResponse {
        HttpResponse {
            status: self.status.unwrap_or(StatusCode(200)),
            version: self.version,
            headers: self.headers,
            body,
        }
    }
}

impl fmt::Debug for HttpResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let headers: Vec<(&str, &str)> = self
            .headers
            .iter()
            .map(|(k, v)| {
                let sensitive = SENSITIVE_HEADERS.iter().any(|s| k.eq_ignore_ascii_case(s));
                let v = if sensitive { "[REDACTED]" } else { v.as_str() };
                (k.as_str(), v)
            })
            .collect();

        f.debug_struct("HttpRequest")
            .field("status", &self.status)
            .field("version", &self.version)
            .field("headers", &headers)
            .field("body", &format_args!("[{} bytes]", self.body.len()))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;

    #[test]
    fn header_case_insensitive() {
        let response = HttpResponse {
            status: StatusCode(200),
            version: String::new(),
            headers: vec![("Content-Type".into(), "text/html".into())],
            body: vec![],
        };
        assert_eq!(response.header("content-type"), Some("text/html"));
        assert_eq!(response.header("CONTENT-TYPE"), Some("text/html"));
        assert_eq!(response.header("Content-Type"), Some("text/html"));
    }

    #[test]
    fn header_missing_returns_none() {
        let response = HttpResponse {
            status: StatusCode(200),
            version: String::new(),
            headers: vec![],
            body: vec![],
        };
        assert_eq!(response.header("x-missing"), None);
    }

    #[test]
    fn header_returns_first_match() {
        let response = HttpResponse {
            status: StatusCode(200),
            version: String::new(),
            headers: vec![
                ("X-Foo".into(), "first".into()),
                ("x-foo".into(), "second".into()),
            ],
            body: vec![],
        };
        assert_eq!(response.header("x-foo"), Some("first"));
    }

    #[test]
    fn builder_stores_headers_lowercase() {
        let mut builder = ResponseBuilder::default();
        builder.header("Content-Type", b"text/plain");
        assert_eq!(builder.headers[0].0, "content-type");
    }

    #[test]
    fn builder_get_header_case_insensitive() {
        let mut builder = ResponseBuilder::default();
        builder.header("Content-Type", b"text/html");
        assert_eq!(builder.get_header("Content-Type"), Some("text/html"));
        assert_eq!(builder.get_header("content-type"), Some("text/html"));
        assert_eq!(builder.get_header("CONTENT-TYPE"), Some("text/html"));
    }

    #[test]
    fn builder_build_defaults_to_200() {
        let response = ResponseBuilder::default().build(vec![]);
        assert_eq!(*response.status, 200);
    }

    #[test]
    fn builder_default_version_is_http11() {
        let response = ResponseBuilder::default().build(vec![]);
        assert_eq!(response.version, "HTTP/1.1");
    }

    #[test]
    fn builder_build_transfers_fields() {
        let mut builder = ResponseBuilder::default();
        builder.status = Some(StatusCode(404));
        builder.header("X-Custom", b"value");
        let response = builder.build(b"not found".to_vec());
        assert_eq!(*response.status, 404);
        assert_eq!(response.header("x-custom"), Some("value"));
        assert_eq!(response.body, b"not found");
    }
}
