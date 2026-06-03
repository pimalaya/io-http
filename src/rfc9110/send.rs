//! Shared output and yield types for HTTP request-response coroutines.
//!
//! Both [`Http10Send`] and [`Http11Send`] return [`HttpSendOutput`] on
//! success and emit [`HttpSendYield`] on every intermediate step.
//!
//! [`Http10Send`]: crate::rfc1945::send::Http10Send
//! [`Http11Send`]: crate::rfc9112::send::Http11Send

use alloc::vec::Vec;

use url::Url;

use crate::{coroutine::HttpYield, rfc9110::response::HttpResponse};

/// Terminal output of a successful HTTP request-response exchange.
///
/// Returned inside `Ok(_)` of `Http10Send` / `Http11Send`'s `Return`
/// type when the server responds with a non-redirect status (or a 3xx
/// whose `Location` header is absent or unparseable).
#[derive(Clone, Debug)]
pub struct HttpSendOutput {
    /// The parsed HTTP response.
    pub response: HttpResponse,
    /// Bytes pre-read past the response body; the caller should feed
    /// these to the next coroutine on the same connection.
    pub remaining: Vec<u8>,
    /// Whether the server indicated the connection can be reused.
    pub keep_alive: bool,
}

/// Per-step yield emitted by [`Http10Send`](crate::rfc1945::send::Http10Send)
/// and [`Http11Send`](crate::rfc9112::send::Http11Send).
///
/// Extends the standard [`HttpYield`](crate::coroutine::HttpYield) with
/// the [`Self::WantsRedirect`] variant: the server responded with a
/// 3xx and a parseable `Location` header. The caller chooses whether
/// to follow the redirect (build a fresh coroutine targeting `url`,
/// possibly on a new connection when `!keep_alive || !same_origin`) or
/// surface it as an error.
#[derive(Debug)]
pub enum HttpSendYield {
    /// Driver should read more bytes from the socket.
    WantsRead,
    /// Driver should write these bytes to the socket; the next resume
    /// typically takes `None`.
    WantsWrite(Vec<u8>),
    /// Server responded with a 3xx redirect.
    WantsRedirect {
        /// Resolved redirect target URL (from the `Location` header).
        url: Url,
        /// The 3xx response received.
        response: HttpResponse,
        /// Whether the server indicated it will keep the connection
        /// open.
        keep_alive: bool,
        /// Whether the redirect stays on the same scheme, host, and
        /// port.
        ///
        /// When `false`, forwarding credentials to the new host without
        /// user consent is inadvisable (RFC 9110 §15.4).
        same_origin: bool,
    },
}

impl From<HttpYield> for HttpSendYield {
    fn from(y: HttpYield) -> Self {
        match y {
            HttpYield::WantsRead => Self::WantsRead,
            HttpYield::WantsWrite(bytes) => Self::WantsWrite(bytes),
        }
    }
}
