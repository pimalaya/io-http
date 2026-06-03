//! Wire-format character constants used by HTTP message parsers.

/// Carriage return byte.
pub const CR: u8 = b'\r';
/// Line feed byte.
pub const LF: u8 = b'\n';
/// Single space byte.
pub const SP: u8 = b' ';

/// `\r\n` line terminator.
pub const CRLF: [u8; 2] = [CR, LF];
/// `\r\n\r\n` headers/body separator.
pub const CRLF_CRLF: [u8; 4] = [CR, LF, CR, LF];
