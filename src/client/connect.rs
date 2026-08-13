//! End-to-end connect for the std client, the half that needs a TLS
//! provider.
//!
//! The module is gated once where it is declared, so nothing inside
//! repeats the feature list.

use alloc::{boxed::Box, string::ToString};

use pimalaya_stream::{std::stream::StreamStd, tls::Tls};
use url::Url;

use crate::client::{HttpClientError, HttpClientStd};

impl HttpClientStd {
    /// Connects to `url` (TLS handshake on `https`), reading ALPN from
    /// `tls.rustls.alpn` (see [`Self::default_alpn`]).
    pub fn connect(url: &Url, tls: &Tls) -> Result<Self, HttpClientError> {
        let host = url
            .host_str()
            .ok_or_else(|| HttpClientError::UrlMissingHost(url.to_string()))?;

        let stream = match url.scheme() {
            "http" => StreamStd::connect_tcp(host, url.port_or_known_default().unwrap_or(80))?,
            "https" => {
                StreamStd::connect_tls(host, url.port_or_known_default().unwrap_or(443), tls)?
            }
            scheme => {
                return Err(HttpClientError::UrlUnsupportedScheme(
                    url.to_string(),
                    scheme.to_string(),
                ));
            }
        };

        Ok(Self {
            stream: Box::new(stream),
        })
    }
}
