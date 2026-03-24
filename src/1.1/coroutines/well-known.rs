use http::{
    header::{self, LOCATION},
    request,
    uri::InvalidUri,
    Request, StatusCode, Uri,
};
use io_stream::io::StreamIo;
use thiserror::Error;

use crate::v1_1::coroutines::send::*;

#[derive(Debug, Error)]
pub enum WellKnownError {
    #[error("Expected a well known redirection, got {0}: {1}")]
    NotRedirected(StatusCode, String),
    #[error("Missing redirect location in HTTP response")]
    MissingLocationHeader,
    #[error("Invalid redirect location in HTTP response: {0}")]
    InvalidLocationHeader(#[source] header::ToStrError, String),
    #[error("Invalid redirect location in HTTP response: {0}")]
    InvalidLocationUri(#[source] InvalidUri, String),

    #[error(transparent)]
    Send(#[from] SendHttpError),
}

/// Send result returned by the coroutine's resume function.
#[derive(Debug)]
pub enum WellKnownResult {
    /// The coroutine wants stream I/O.
    Io { io: StreamIo },
    /// The coroutine has successfully terminated its execution.
    Ok { uri: Uri, keep_alive: bool },
    /// The coroutine encountered an error.
    Err { err: WellKnownError },
}

#[derive(Debug)]
pub struct WellKnown(SendHttp);

impl WellKnown {
    pub fn prepare_request(
        uri: impl AsRef<str>,
        service: impl AsRef<str>,
    ) -> Result<request::Builder, InvalidUri> {
        let mut parts = Uri::try_from(uri.as_ref())?.into_parts();

        let path = format!("/.well-known/{}", service.as_ref());
        parts.path_and_query = Some(path.parse()?);

        Ok(Request::get(parts))
    }

    pub fn new(request: request::Builder) -> Result<Self, http::Error> {
        Ok(Self(SendHttp::new(request.body(vec![])?)))
    }

    pub fn resume(&mut self, arg: Option<StreamIo>) -> WellKnownResult {
        let ok = match self.0.resume(arg) {
            SendHttpResult::Io(io) => return WellKnownResult::Io { io },
            SendHttpResult::Ok(ok) => ok,
            SendHttpResult::Err(err) => {
                return WellKnownResult::Err { err: err.into() };
            }
        };

        let status = ok.response.status();

        if !status.is_redirection() {
            let body = String::from_utf8_lossy(ok.response.body()).to_string();
            let err = WellKnownError::NotRedirected(status, body);
            return WellKnownResult::Err { err };
        }

        let Some(uri) = ok.response.headers().get(LOCATION) else {
            let err = WellKnownError::MissingLocationHeader;
            return WellKnownResult::Err { err };
        };

        let uri = match uri.to_str() {
            Ok(uri) => uri,
            Err(err) => {
                let uri = format!("{uri:?}");
                let err = WellKnownError::InvalidLocationHeader(err, uri);
                return WellKnownResult::Err { err };
            }
        };

        let uri: Uri = match uri.parse() {
            Ok(uri) => uri,
            Err(err) => {
                let uri = uri.to_string();
                let err = WellKnownError::InvalidLocationUri(err, uri);
                return WellKnownResult::Err { err };
            }
        };

        let same_scheme = if let Some(scheme) = uri.scheme() {
            ok.request.uri().scheme() == Some(scheme)
        } else {
            true
        };

        let same_authority = if let Some(auth) = uri.authority() {
            ok.request.uri().authority() == Some(auth)
        } else {
            true
        };

        let keep_alive = ok.keep_alive && same_scheme && same_authority;

        WellKnownResult::Ok { uri, keep_alive }
    }
}
