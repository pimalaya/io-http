//! W3C Server-Sent Events client.
//!
//! The frame parser is decoupled from the body transport: bytes come in via
//! [`frame::SseFrameParser`]'s `HttpCoroutine::resume`, dispatched events come
//! out as [`frame::SseFrame`].
//!
//! [Server-Sent Events]: https://html.spec.whatwg.org/multipage/server-sent-events.html

pub mod frame;
