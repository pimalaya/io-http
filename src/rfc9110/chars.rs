//! Wire-format character constants used by HTTP message parsers.

pub const CR: u8 = b'\r';
pub const LF: u8 = b'\n';
pub const SP: u8 = b' ';

pub const CRLF: [u8; 2] = [CR, LF];
pub const CRLF_CRLF: [u8; 4] = [CR, LF, CR, LF];
