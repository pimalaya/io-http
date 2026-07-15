//! HTTP/1.0 request serialisation onto the wire ([RFC 1945 §4]).
//!
//! [RFC 1945 §4]: https://www.rfc-editor.org/rfc/rfc1945#section-4

use alloc::{format, vec::Vec};

use crate::{
    rfc1945::version::HTTP_10,
    rfc9110::{
        chars::{CRLF, CRLF_CRLF, SP},
        headers::HTTP_CONTENT_LENGTH,
        request::HttpRequest,
    },
};

impl HttpRequest {
    /// Serialises this request as an HTTP/1.0 message; `Content-Length` is
    /// regenerated from the body and any existing copy is dropped.
    pub fn to_http_10_vec(&self) -> Vec<u8> {
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

        bytes
    }
}
