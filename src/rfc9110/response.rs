//! HTTP response type ([RFC 9110 §15]).
//!
//! [RFC 9110 §15]: https://www.rfc-editor.org/rfc/rfc9110#section-15

use core::fmt;

use alloc::{string::String, vec::Vec};

#[cfg(feature = "client")]
use crate::rfc9110::headers::HTTP_CONTENT_LENGTH;
use crate::rfc9110::{headers::HttpHeadersDebug, status::HttpStatusCode};

/// An incoming HTTP response. Header names are stored lowercase.
///
/// Its [`fmt::Debug`] redacts the sensitive headers and prints the
/// body as its length.
#[derive(Clone)]
pub struct HttpResponse {
    /// The response status code.
    pub status: HttpStatusCode,
    /// The protocol version token found on the status line.
    pub version: String,
    /// The response headers, in order, names lowercased.
    pub headers: Vec<(String, String)>,
    /// The response body, decoded when chunked.
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

    #[cfg(feature = "client")]
    /// The elements of every header matching `name`, split on commas.
    pub(crate) fn header_list(&self, name: &str) -> impl Iterator<Item = &str> + Clone {
        self.headers
            .iter()
            .filter(move |(k, _)| k.eq_ignore_ascii_case(name))
            .flat_map(|(_, v)| v.split(','))
            .map(str::trim)
    }

    #[cfg(feature = "client")]
    /// The content length, `None` without the header. Fails with the
    /// header values when one is invalid or two differ.
    pub(crate) fn content_length(&self) -> Result<Option<usize>, String> {
        let values = self.header_list(HTTP_CONTENT_LENGTH);
        let mut len = None;

        for value in values.clone() {
            let is_digits = value.bytes().all(|b| b.is_ascii_digit());
            let n = is_digits.then(|| value.parse::<usize>().ok()).flatten();

            match (n, len) {
                (Some(n), None) => len = Some(n),
                (Some(n), Some(len)) if n == len => {}
                _ => return Err(values.collect::<Vec<_>>().join(", ")),
            }
        }

        Ok(len)
    }
}

impl fmt::Debug for HttpResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpResponse")
            .field("status", &self.status)
            .field("version", &self.version)
            .field("headers", &HttpHeadersDebug(&self.headers))
            .field("body", &format_args!("[{} bytes]", self.body.len()))
            .finish()
    }
}

#[cfg(feature = "client")]
/// Incremental builder for [`HttpResponse`].
#[derive(Clone)]
pub(crate) struct HttpResponseBuilder {
    pub(crate) status: Option<HttpStatusCode>,
    pub(crate) version: String,
    pub(crate) headers: Vec<(String, String)>,
}

#[cfg(feature = "client")]
impl Default for HttpResponseBuilder {
    fn default() -> Self {
        Self {
            status: None,
            version: "HTTP/1.1".into(),
            headers: Vec::new(),
        }
    }
}

#[cfg(feature = "client")]
impl HttpResponseBuilder {
    pub(crate) fn header(&mut self, name: &str, value: &[u8]) {
        let value = String::from_utf8_lossy(value).into_owned();
        self.headers.push((name.to_lowercase(), value));
    }

    pub(crate) fn build(self, body: Vec<u8>) -> HttpResponse {
        HttpResponse {
            status: self.status.unwrap_or(HttpStatusCode(200)),
            version: self.version,
            headers: self.headers,
            body,
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::{format, vec};

    use crate::rfc9110::response::*;

    #[test]
    fn header_case_insensitive() {
        let response = HttpResponse {
            status: HttpStatusCode(200),
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
            status: HttpStatusCode(200),
            version: String::new(),
            headers: vec![],
            body: vec![],
        };
        assert_eq!(response.header("x-missing"), None);
    }

    #[test]
    fn header_returns_first_match() {
        let response = HttpResponse {
            status: HttpStatusCode(200),
            version: String::new(),
            headers: vec![
                ("X-Foo".into(), "first".into()),
                ("x-foo".into(), "second".into()),
            ],
            body: vec![],
        };
        assert_eq!(response.header("x-foo"), Some("first"));
    }

    #[cfg(feature = "client")]
    #[test]
    fn builder_stores_headers_lowercase() {
        let mut builder = HttpResponseBuilder::default();
        builder.header("Content-Type", b"text/plain");
        assert_eq!(builder.headers[0].0, "content-type");
    }

    #[cfg(feature = "client")]
    #[test]
    fn builder_build_defaults_to_200() {
        let response = HttpResponseBuilder::default().build(vec![]);
        assert_eq!(*response.status, 200);
    }

    #[cfg(feature = "client")]
    #[test]
    fn builder_default_version_is_http11() {
        let response = HttpResponseBuilder::default().build(vec![]);
        assert_eq!(response.version, "HTTP/1.1");
    }

    #[cfg(feature = "client")]
    #[test]
    fn builder_build_transfers_fields() {
        let mut builder = HttpResponseBuilder {
            status: Some(HttpStatusCode(404)),
            ..Default::default()
        };
        builder.header("X-Custom", b"value");
        let response = builder.build(b"not found".to_vec());
        assert_eq!(*response.status, 404);
        assert_eq!(response.header("x-custom"), Some("value"));
        assert_eq!(response.body, b"not found");
    }

    #[test]
    fn debug_redacts_sensitive_headers_and_body() {
        let response = HttpResponse {
            status: HttpStatusCode(200),
            version: "HTTP/1.1".into(),
            headers: vec![
                ("set-cookie".into(), "sid=abc123".into()),
                ("content-type".into(), "application/json".into()),
            ],
            body: br#"{"access_token":"secret-token"}"#.to_vec(),
        };
        let debug = format!("{response:?}");

        assert!(!debug.contains("abc123"), "got {debug}");
        assert!(!debug.contains("secret-token"), "got {debug}");
        assert!(debug.contains("application/json"), "got {debug}");
        assert!(debug.contains("body: [31 bytes]"), "got {debug}");
    }
}
