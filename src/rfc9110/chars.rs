//! Wire-format character constants used by HTTP message parsers.

/// Carriage return.
pub const CR: u8 = b'\r';
/// Line feed.
pub const LF: u8 = b'\n';
/// Space.
pub const SP: u8 = b' ';

/// Line terminator: carriage return followed by line feed.
pub const CRLF: [u8; 2] = [CR, LF];
/// Head terminator: two consecutive line terminators.
pub const CRLF_CRLF: [u8; 4] = [CR, LF, CR, LF];
