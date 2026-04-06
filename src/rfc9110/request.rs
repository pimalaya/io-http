//! HTTP request type (RFC 9110 §9).

use alloc::{
    string::{String, ToString},
    vec::Vec,
};
use core::fmt;

use url::Url;

use crate::rfc9110::headers::SENSITIVE_HEADERS;

/// An outgoing HTTP request.
#[derive(Clone)]
pub struct HttpRequest {
    /// HTTP method (e.g. `"GET"`, `"POST"`).
    pub method: String,
    /// Request target URL.
    pub url: Url,
    /// Request headers as `(name, value)` pairs.
    pub headers: Vec<(String, String)>,
    /// Request body bytes.
    pub body: Vec<u8>,
}

impl HttpRequest {
    /// Creates a new GET request to the given URL with no headers or body.
    pub fn get(url: Url) -> Self {
        Self {
            method: "GET".into(),
            url,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    /// Appends a header.
    pub fn header(mut self, name: impl ToString, value: impl ToString) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    /// Sets the request body.
    pub fn body(mut self, body: Vec<u8>) -> Self {
        self.body = body;
        self
    }
}

impl fmt::Debug for HttpRequest {
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
            .field("method", &self.method)
            .field("url", &self.url.as_str())
            .field("headers", &headers)
            .field("body", &format_args!("[{} bytes]", self.body.len()))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use alloc::format;
    use url::Url;

    use super::*;

    #[test]
    fn get_method_and_empty_body() {
        let url = Url::parse("http://example.com/path").unwrap();
        let req = HttpRequest::get(url);
        assert_eq!(req.method, "GET");
        assert!(req.body.is_empty());
        assert!(req.headers.is_empty());
    }

    #[test]
    fn header_appended() {
        let url = Url::parse("http://example.com/").unwrap();
        let req = HttpRequest::get(url)
            .header("Host", "example.com")
            .header("Accept", "text/html");
        assert_eq!(req.headers.len(), 2);
        assert_eq!(req.headers[0], ("Host".into(), "example.com".into()));
        assert_eq!(req.headers[1], ("Accept".into(), "text/html".into()));
    }

    #[test]
    fn body_replaces() {
        let url = Url::parse("http://example.com/").unwrap();
        let req = HttpRequest::get(url).body(b"hello".to_vec());
        assert_eq!(req.body, b"hello");
    }

    #[test]
    fn debug_redacts_sensitive_headers() {
        let url = Url::parse("http://example.com/").unwrap();
        let req = HttpRequest::get(url)
            .header("Host", "example.com")
            .header("Authorization", "Bearer secret-token")
            .header("Cookie", "session=abc123");
        let debug = format!("{req:?}");
        assert!(debug.contains("[REDACTED]"), "expected redaction marker");
        assert!(
            !debug.contains("secret-token"),
            "token must not appear in debug"
        );
        assert!(
            !debug.contains("abc123"),
            "cookie value must not appear in debug"
        );
        assert!(
            debug.contains("example.com"),
            "non-sensitive header value must appear"
        );
    }
}
