#![no_std]
#![deny(missing_docs)]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! # io-http
//!
//! I/O-free HTTP client coroutines. Every network exchange is a
//! resumable state machine that emits read and write requests instead
//! of performing I/O itself: the caller owns the socket and pumps the
//! coroutine with the bytes it read, whatever the runtime (blocking,
//! async, in-memory tests). The `client` feature ships a ready-made
//! std-blocking pump for callers who just want a working client.
//!
//! ## Layout: one folder per RFC
//!
//! io-http covers HTTP in general; today HTTP/1.0 and HTTP/1.1 are
//! implemented. The source tree mirrors how the HTTP specification
//! itself is split, one module per RFC, so the RFC number is the
//! version discriminator: a future version (HTTP/2, HTTP/3) would slot
//! in as its own RFC modules alongside the existing ones.
//! [`rfc9110`] holds the version-agnostic semantics shared by every
//! wire format: the request and response types, status codes, header
//! name constants, authentication challenge parsing, and the output
//! and yield types common to both send coroutines. [`rfc1945`]
//! implements the HTTP/1.0 wire protocol and [`rfc9112`] the HTTP/1.1
//! one: each ships its request serialiser and send coroutine, and
//! HTTP/1.1 adds the response-head parser plus two chunked
//! transfer-coding decoders (whole-body and streaming).
//!
//! Around the wire protocols, [`rfc6750`] and [`rfc7617`] provide the
//! bearer and basic authorization header helpers, with secrets
//! redacted from debug output; [`rfc8615`] wraps the HTTP/1.1 send
//! coroutine into a well-known URI discovery coroutine surfacing the
//! redirect target.
//!
//! Two modules span the RFC modules and therefore live at the crate
//! root: [`coroutine`] defines the coroutine contract every state
//! machine implements, and [`sse`] parses W3C Server-Sent Events
//! frames (a WHATWG HTML Living Standard, not an RFC, hence its own
//! name). The optional [`client`] module (`client` feature) is the
//! std-blocking pump: a light client wrapping any stream you opened
//! yourself, or a full client opening the TCP/TLS connection itself
//! when one of the TLS features is enabled.
//!
//! ## The coroutine contract
//!
//! Every coroutine implements [`coroutine::HttpCoroutine`]: a resume
//! method taking the bytes read since the last step and returning
//! either an intermediate yield or a terminal completion. Standard
//! coroutines yield the shared read/write requests of
//! [`coroutine::HttpYield`]; richer coroutines declare their own yield
//! type, like the send coroutines surfacing 3xx redirects to the
//! caller instead of following them, or the streaming decoders
//! yielding one frame at a time. Completion carries a per-coroutine
//! output or error; the [`http_try`] macro chains an inner coroutine
//! step inside an outer resume, re-yielding and short-circuiting like
//! the question mark operator.
//!
//! ## Conventions
//!
//! The crate is no_std with alloc; std only enters behind the `client`
//! feature. Public items carry the version-agnostic `Http` prefix when
//! they belong to the shared semantics, and the version-scoped
//! `Http10` / `Http11` prefix when they are tied to a wire format; the
//! `Sse` family keeps its own deliberate namespace. Logging follows
//! the library rules: state changes at debug level, in-process steps
//! and data dumps at trace level.

extern crate alloc;
#[cfg(feature = "client")]
extern crate std;

#[cfg(feature = "client")]
pub mod client;
pub mod coroutine;
pub mod rfc1945;
pub mod rfc6750;
pub mod rfc7617;
pub mod rfc8615;
pub mod rfc9110;
pub mod rfc9112;
pub mod sse;
