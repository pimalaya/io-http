//! HTTP/1.1 request serialisation onto the wire ([RFC 9112 §3]).
//!
//! [RFC 9112 §3]: https://www.rfc-editor.org/rfc/rfc9112#section-3

use alloc::{format, vec::Vec};

use crate::{
    rfc9110::{
        chars::{CRLF, CRLF_CRLF, SP},
        headers::CONTENT_LENGTH,
        request::HttpRequest,
    },
    rfc9112::version::HTTP_11,
};

impl HttpRequest {
    /// Serialises this request as an HTTP/1.1 message; `Content-Length`
    /// is regenerated from the body and any existing copy is dropped.
    pub fn to_http_11_vec(&self) -> Vec<u8> {
        let mut bytes = Vec::new();

        bytes.extend(self.method.as_bytes());
        bytes.push(SP);
        bytes.extend(self.url.path().as_bytes());

        if let Some(q) = self.url.query() {
            bytes.extend(b"?");
            bytes.extend(q.as_bytes());
        }

        bytes.push(SP);
        bytes.extend(HTTP_11.as_bytes());
        bytes.extend(CRLF);

        for (key, val) in &self.headers {
            if key.eq_ignore_ascii_case(CONTENT_LENGTH) {
                continue;
            }

            bytes.extend(key.as_bytes());
            bytes.extend(b": ");
            bytes.extend(val.as_bytes());
            bytes.extend(CRLF);
        }

        let body_len = format!("{}", self.body.len());
        bytes.extend(CONTENT_LENGTH.as_bytes());
        bytes.extend(b": ");
        bytes.extend(body_len.as_bytes());
        bytes.extend(CRLF_CRLF);
        bytes.extend(&self.body);

        bytes
    }
}
