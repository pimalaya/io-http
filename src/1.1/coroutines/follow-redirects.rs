use http::{header::LOCATION, Uri};
use io_stream::io::StreamIo;
use thiserror::Error;

use super::send::{Send, SendError, SendOk, SendResult};

#[derive(Debug, Error)]
pub enum FollowRedirectsError {
    #[error("Missing redirect location in HTTP response")]
    MissingLocationHeader,
    #[error("Invalid redirect location in HTTP response: {0}")]
    InvalidLocationHeader(#[source] http::header::ToStrError, String),
    #[error("Invalid redirect location in HTTP response: {0}")]
    InvalidLocationUri(#[source] http::uri::InvalidUri, String),
    #[error("Redirected too many times")]
    TooManyRedirects,

    #[error(transparent)]
    Send(#[from] SendError),
}

/// Send result returned by the coroutine's resume function.
#[derive(Debug)]
pub enum FollowRedirectsResult {
    /// The coroutine has successfully terminated its execution.
    Ok(SendOk),
    /// The coroutine encountered an error.
    Err(FollowRedirectsError),
    /// The coroutine wants stream I/O.
    Io(StreamIo),
    /// The coroutine wants I/O to re-create the stream.
    ///
    /// This case happens when a redirection response is received with
    /// an absolute `Location` URI (which implies that the current
    /// stream cannot be used anymore).
    Reset(Uri),
}

#[derive(Debug)]
pub struct FollowRedirects {
    send: Send,
    pub remaining: u8,
}

impl FollowRedirects {
    pub fn new(send: Send) -> Self {
        Self { send, remaining: 4 }
    }

    pub fn resume(&mut self, mut input: Option<StreamIo>) -> FollowRedirectsResult {
        loop {
            if self.remaining == 0 {
                break FollowRedirectsResult::Err(FollowRedirectsError::TooManyRedirects);
            }

            let mut ok = match self.send.resume(input.take()) {
                SendResult::Ok(ok) => ok,
                SendResult::Err(err) => break FollowRedirectsResult::Err(err.into()),
                SendResult::Io(io) => break FollowRedirectsResult::Io(io),
            };

            if ok.response.status().is_redirection() {
                let Some(uri) = ok.response.headers().get(LOCATION) else {
                    return FollowRedirectsResult::Err(FollowRedirectsError::MissingLocationHeader);
                };

                let uri = match uri.to_str() {
                    Ok(uri) => uri,
                    Err(err) => {
                        let err =
                            FollowRedirectsError::InvalidLocationHeader(err, format!("{uri:?}"));
                        return FollowRedirectsResult::Err(err);
                    }
                };

                let uri: Uri = match uri.parse() {
                    Ok(uri) => uri,
                    Err(err) => {
                        let err = FollowRedirectsError::InvalidLocationUri(err, uri.to_string());
                        return FollowRedirectsResult::Err(err);
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

                self.send = Send::new(ok.request);
                self.remaining -= 1;

                if !ok.keep_alive || !same_scheme || !same_authority {
                    return FollowRedirectsResult::Reset(uri);
                }

                continue;
            }

            break FollowRedirectsResult::Ok(ok);
        }
    }
}
