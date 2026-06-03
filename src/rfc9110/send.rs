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
#[derive(Clone, Debug)]
pub struct HttpSendOutput {
    pub response: HttpResponse,
    pub remaining: Vec<u8>,
    pub keep_alive: bool,
}

/// Per-step yield emitted by `Http10Send` / `Http11Send`; adds
/// [`Self::WantsRedirect`] to the standard [`HttpYield`] for 3xx responses.
#[derive(Debug)]
pub enum HttpSendYield {
    WantsRead,
    WantsWrite(Vec<u8>),
    WantsRedirect {
        url: Url,
        response: HttpResponse,
        keep_alive: bool,
        /// `false` when the redirect crosses scheme/host/port; do not
        /// forward credentials without user consent (RFC 9110 §15.4).
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
