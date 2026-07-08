//! HTTP/1.1 request serialisation onto the wire ([RFC 9112 §3]).
//!
//! [RFC 9112 §3]: https://www.rfc-editor.org/rfc/rfc9112#section-3

use alloc::{format, vec::Vec};

use crate::{
    rfc9110::{
        chars::{CRLF, CRLF_CRLF, SP},
        headers::{CONTENT_LENGTH, HOST},
        request::HttpRequest,
    },
    rfc9112::version::HTTP_11,
};

impl HttpRequest {
    /// Serialises this request as an HTTP/1.1 message; `Content-Length`
    /// is regenerated from the body and any existing copy is dropped,
    /// and a `Host` is generated from the URL (RFC 9112 §3.2) when the
    /// caller did not set one.
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

        let has_host = self
            .headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case(HOST));
        if !has_host {
            if let Some(host) = self.url.host_str() {
                bytes.extend(HOST.as_bytes());
                bytes.extend(b": ");
                bytes.extend(host.as_bytes());
                // NOTE: `Url` drops the port when it is the scheme
                // default, which is exactly when Host must omit it too.
                if let Some(port) = self.url.port() {
                    bytes.extend(format!(":{port}").as_bytes());
                }
                bytes.extend(CRLF);
            }
        }

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

#[cfg(test)]
mod tests {
    use alloc::string::String;

    use url::Url;

    use crate::rfc9110::request::HttpRequest;

    fn serialize(req: &HttpRequest) -> String {
        String::from_utf8(req.to_http_11_vec()).unwrap()
    }

    #[test]
    fn host_generated_from_url() {
        let req = HttpRequest::get(Url::parse("https://example.com/x").unwrap());
        assert!(serialize(&req).contains("host: example.com\r\n"));
    }

    #[test]
    fn host_keeps_non_default_port() {
        let req = HttpRequest::get(Url::parse("https://example.com:8843/").unwrap());
        assert!(serialize(&req).contains("host: example.com:8843\r\n"));
    }

    #[test]
    fn host_omits_default_port() {
        let req = HttpRequest::get(Url::parse("https://example.com:443/").unwrap());
        assert!(serialize(&req).contains("host: example.com\r\n"));
    }

    #[test]
    fn host_not_duplicated_when_set_by_caller() {
        let req = HttpRequest::get(Url::parse("https://example.com/").unwrap())
            .header("Host", "override.example");
        let message = serialize(&req);
        assert!(message.contains("Host: override.example\r\n"));
        assert!(!message.contains("host: example.com\r\n"));
    }
}
