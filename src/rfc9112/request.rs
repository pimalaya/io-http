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
            // skip content-length, as it is automatically
            // generated below
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
