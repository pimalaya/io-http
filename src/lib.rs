#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

//! The [HTTP flows] project is a set of libraries to manage HTTP
//! streams in a I/O-agnostic way. It is highly recommended that you
//! read first about the project in order to understand `http-lib`.
//!
//! This library gathers all the I/O-free part of the project.
//!
//! [HTTP flows]: https://github.com/pimalaya/http

pub mod coroutines;
mod response;
#[path = "1.1/mod.rs"]
pub mod v1_1;

#[doc(inline)]
pub use self::response::Response;
