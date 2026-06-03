//! HTTP response type ([RFC 9110 §15]).
//!
//! [RFC 9110 §15]: https://www.rfc-editor.org/rfc/rfc9110#section-15

use core::{fmt, str};

use alloc::{borrow::ToOwned, format, string::String, vec::Vec};

use crate::rfc9110::{headers::SENSITIVE_HEADERS, status::StatusCode};

/// An incoming HTTP response. Header names are stored lowercase.
#[derive(Clone)]
pub struct HttpResponse {
    pub status: StatusCode,
    pub version: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    /// Returns the first header matching `name` (case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
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

        f.debug_struct("HttpResponse")
            .field("status", &self.status)
            .field("version", &self.version)
            .field("headers", &headers)
            .field(
                "body",
                &match str::from_utf8(&self.body) {
                    Ok(body) => body.to_owned(),
                    Err(_) => format!("[{} BYTES]", self.body.len()),
                },
            )
            .finish()
    }
}

/// Incremental builder for [`HttpResponse`].
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
    pub(crate) fn header(&mut self, name: &str, value: &[u8]) {
        let value = String::from_utf8_lossy(value).into_owned();
        self.headers.push((name.to_lowercase(), value));
    }

    pub(crate) fn get_header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub(crate) fn build(self, body: Vec<u8>) -> HttpResponse {
        HttpResponse {
            status: self.status.unwrap_or(StatusCode(200)),
            version: self.version,
            headers: self.headers,
            body,
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use crate::rfc9110::response::*;

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
