//! W3C Server-Sent Events client (HTML Living Standard,
//! "Server-sent events" section). The frame parser is decoupled from
//! the body transport: bytes come in via [`frame::SseFrameParser`]'s
//! `HttpCoroutine::resume`, dispatched events come out as
//! [`frame::SseFrame`].

pub mod frame;
