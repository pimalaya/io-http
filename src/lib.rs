#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![doc = include_str!("../README.md")]
#![no_std]
extern crate alloc;

pub mod rfc1945;
pub mod rfc6750;
pub mod rfc7617;
pub mod rfc8615;
pub mod rfc9110;
pub mod rfc9112;
