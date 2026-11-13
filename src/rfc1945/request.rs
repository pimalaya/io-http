//! HTTP/1.0 request serialisation onto the wire ([RFC 1945 §4]).
//!
//! [RFC 1945 §4]: https://www.rfc-editor.org/rfc/rfc1945#section-4

use alloc::{format, vec::Vec};

use crate::{
    rfc1945::version::HTTP_10,
    rfc9110::{
        chars::{CRLF, CRLF_CRLF, SP},
        headers::HTTP_CONTENT_LENGTH,
        request::{HttpRequest, HttpRequestError},
    },
};

impl HttpRequest {
    /// Serialises this request as an HTTP/1.0 message; `Content-Length` is
    /// regenerated from the body and any existing copy is dropped. Fails
    /// on a method, a header name or a header value that would add a
    /// line to the message.
    pub fn to_http_10_vec(&self) -> Result<Vec<u8>, HttpRequestError> {
        self.validate()?;

        let mut bytes = Vec::new();

        bytes.extend(self.method.as_bytes());
        bytes.push(SP);
        bytes.extend(self.url.path().as_bytes());

        if let Some(q) = self.url.query() {
            bytes.extend(b"?");
            bytes.extend(q.as_bytes());
        }

        bytes.push(SP);
        bytes.extend(HTTP_10.as_bytes());
        bytes.extend(CRLF);

        for (key, val) in &self.headers {
            if key.eq_ignore_ascii_case(HTTP_CONTENT_LENGTH) {
                continue;
            }

            bytes.extend(key.as_bytes());
            bytes.extend(b": ");
            bytes.extend(val.as_bytes());
            bytes.extend(CRLF);
        }

        let body_len = format!("{}", self.body.len());
        bytes.extend(HTTP_CONTENT_LENGTH.as_bytes());
        bytes.extend(b": ");
        bytes.extend(body_len.as_bytes());
        bytes.extend(CRLF_CRLF);
        bytes.extend(&self.body);

        Ok(bytes)
    }
}
