//! I/O-free coroutine to follow HTTP redirections.

use http::{header::LOCATION, Uri};
use io_stream::io::StreamIo;
use thiserror::Error;

use super::send::{SendHttp, SendHttpError, SendHttpOk, SendHttpResult};

/// Errors that can occur during the coroutine progression.
#[derive(Debug, Error)]
pub enum FollowHttpRedirectsError {
    /// The Location HTTP response header is missing.
    #[error("Missing redirect location in HTTP response")]
    MissingLocationHeader,
    /// The Location HTTP response header is not a valid path.
    #[error("Invalid redirect location in HTTP response: {0}")]
    InvalidLocationHeader(#[source] http::header::ToStrError, String),
    /// The Location HTTP response header is not a valid URI.
    #[error("Invalid redirect location in HTTP response: {0}")]
    InvalidLocationUri(#[source] http::uri::InvalidUri, String),
    /// The coroutine has redirected too many times.
    #[error("Redirected too many times")]
    TooManyRedirects,

    #[error(transparent)]
    SendHttp(#[from] SendHttpError),
}

/// Send result returned by the coroutine's resume function.
#[derive(Debug)]
pub enum FollowHttpRedirectsResult {
    /// The coroutine has successfully terminated its execution.
    Ok(SendHttpOk),
    /// The coroutine encountered an error.
    Err(FollowHttpRedirectsError),
    /// The coroutine wants stream I/O.
    Io(StreamIo),
    /// The coroutine wants I/O to re-create the stream.
    ///
    /// This case happens when a redirection response is received with
    /// an absolute `Location` URI (which implies that the current
    /// stream cannot be used anymore).
    Reset(Uri),
}

/// I/O-free coroutine to follow HTTP redirections.
#[derive(Debug)]
pub struct FollowHttpRedirects {
    send: SendHttp,
    pub remaining: u8,
}

impl FollowHttpRedirects {
    /// Creates a new coroutine from the given [`SendHttp`]
    /// sub-coroutine.
    pub fn new(send: SendHttp) -> Self {
        Self { send, remaining: 4 }
    }

    /// Makes the coroutine progress.
    pub fn resume(&mut self, mut arg: Option<StreamIo>) -> FollowHttpRedirectsResult {
        loop {
            if self.remaining == 0 {
                break FollowHttpRedirectsResult::Err(FollowHttpRedirectsError::TooManyRedirects);
            }

            let mut ok = match self.send.resume(arg.take()) {
                SendHttpResult::Ok(ok) => ok,
                SendHttpResult::Err(err) => break FollowHttpRedirectsResult::Err(err.into()),
                SendHttpResult::Io(io) => break FollowHttpRedirectsResult::Io(io),
            };

            if ok.response.status().is_redirection() {
                let Some(uri) = ok.response.headers().get(LOCATION) else {
                    return FollowHttpRedirectsResult::Err(
                        FollowHttpRedirectsError::MissingLocationHeader,
                    );
                };

                let uri = match uri.to_str() {
                    Ok(uri) => uri,
                    Err(err) => {
                        let err = FollowHttpRedirectsError::InvalidLocationHeader(
                            err,
                            format!("{uri:?}"),
                        );
                        return FollowHttpRedirectsResult::Err(err);
                    }
                };

                let uri: Uri = match uri.parse() {
                    Ok(uri) => uri,
                    Err(err) => {
                        let err =
                            FollowHttpRedirectsError::InvalidLocationUri(err, uri.to_string());
                        return FollowHttpRedirectsResult::Err(err);
                    }
                };

                let same_scheme = ok.request.uri().scheme() == uri.scheme();
                let same_authority = ok.request.uri().authority() == uri.authority();

                let (mut request_parts, body) = ok.request.into_parts();
                let mut cur_uri_parts = request_parts.uri.into_parts();
                let uri_parts = uri.into_parts();

                if let Some(scheme) = uri_parts.scheme {
                    cur_uri_parts.scheme = Some(scheme);
                }

                if let Some(authority) = uri_parts.authority {
                    cur_uri_parts.authority = Some(authority);
                }

                cur_uri_parts.path_and_query = uri_parts.path_and_query;

                request_parts.uri = Uri::from_parts(cur_uri_parts).unwrap();
                ok.request = http::request::Request::from_parts(request_parts, body);
                let uri = ok.request.uri().clone();

                self.send = SendHttp::new(ok.request);
                self.remaining -= 1;

                if !ok.keep_alive || !same_scheme || !same_authority {
                    return FollowHttpRedirectsResult::Reset(uri);
                }

                continue;
            }

            break FollowHttpRedirectsResult::Ok(ok);
        }
    }
}
