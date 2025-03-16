use base64::{prelude::BASE64_STANDARD, Engine};
use chrono::Utc;
use secrecy::{ExposeSecret, SecretString};

pub const CR: u8 = b'\r';
pub const CRLF: [u8; 2] = [CR, LF];
pub const LF: u8 = b'\n';
pub const SP: u8 = b' ';

#[derive(Clone, Debug, Default)]
pub struct Request {
    bytes: Vec<u8>,
}

impl Request {
    pub fn new(method: &str, uri: &str, version: &str) -> Self {
        let mut bytes = Vec::new();

        bytes.extend(method.as_bytes());
        bytes.push(SP);
        bytes.extend(uri.as_bytes());
        bytes.push(SP);
        bytes.extend(b"HTTP/");
        bytes.extend(version.as_bytes());
        bytes.extend(CRLF);

        // bytes.extend(b"Date: ");
        // bytes.extend(Utc::now().format("%a, %d %b %Y %T").to_string().as_bytes());
        // bytes.extend(b" GMT");
        // bytes.extend(CRLF);

        Self { bytes }
    }

    pub fn header(mut self, key: &str, value: &str) -> Self {
        self.bytes.extend(key.as_bytes());
        self.bytes.extend(b": ");
        self.bytes.extend(value.as_bytes());
        self.bytes.extend(CRLF);
        self
    }

    pub fn basic_auth(mut self, user: &str, pass: &SecretString) -> Self {
        let pass = pass.expose_secret();
        let auth = BASE64_STANDARD.encode(format!("{user}:{pass}"));
        self.bytes.extend(b"Authorization: Basic ");
        self.bytes.extend(auth.as_bytes());
        self.bytes.extend(CRLF);
        self
    }

    pub fn body(mut self, body: &str) -> Self {
        self.bytes.extend(b"Content-Length: ");
        self.bytes.extend(body.len().to_string().as_bytes());
        self.bytes.extend(CRLF);
        self.bytes.extend(CRLF);
        self.bytes.extend(body.as_bytes());
        self
    }
}

impl AsRef<[u8]> for Request {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

impl From<Request> for Vec<u8> {
    fn from(request: Request) -> Self {
        request.bytes
    }
}

impl IntoIterator for Request {
    type Item = u8;
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.bytes.into_iter()
    }
}
