//! I/O-free coroutine decoding a W3C [Server-Sent Events] stream.
//! Line-oriented: the parser never completes on its own, it fails once
//! a line or an event exceeds its limit, and the caller stops when the
//! body stream closes.
//!
//! [Server-Sent Events]: https://html.spec.whatwg.org/multipage/server-sent-events.html

use core::{convert::Infallible, mem, str};

use alloc::{
    string::{String, ToString},
    vec::Vec,
};

use log::trace;
use thiserror::Error;

use crate::{
    coroutine::*,
    rfc9110::chars::{CR, LF},
    scan::position,
};

/// Default value of [`SseFrameParser::max_line_len`]: 64 KiB.
pub const SSE_DEFAULT_MAX_LINE_LEN: usize = 64 * 1024;

/// Default value of [`SseFrameParser::max_event_len`]: 1 MiB.
pub const SSE_DEFAULT_MAX_EVENT_LEN: usize = 1024 * 1024;

/// The UTF-8 byte order mark, ignored once at the stream start.
const BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];

/// Failure causes of the Server-Sent Events frame parser.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum SseFrameParserError {
    /// A line exceeds [`SseFrameParser::max_line_len`].
    #[error("SSE parse frame failed: line exceeds {0} bytes")]
    LineTooLarge(usize),
    /// The data of an event exceeds [`SseFrameParser::max_event_len`].
    #[error("SSE parse frame failed: event exceeds {0} bytes")]
    EventTooLarge(usize),
}

/// One dispatched Server-Sent Event. `event` is `None` when the
/// `event:` field was absent; `data` has the trailing newline stripped.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SseFrame {
    /// The event type, when the stream named one.
    pub event: Option<String>,
    /// The event payload, data lines joined by newlines.
    pub data: String,
    /// The last event id in effect when the event was dispatched.
    pub id: Option<String>,
    /// The reconnection delay in milliseconds, when the stream set one.
    pub retry: Option<u64>,
}

/// Per-step yield emitted by [`SseFrameParser`].
#[derive(Debug)]
pub enum SseFrameParserYield {
    /// One dispatched event.
    Frame(SseFrame),
    /// The parser wants more body bytes handed back on the next
    /// resume.
    WantsBytes,
}

/// I/O-free Server-Sent Events frame parser.
#[derive(Debug)]
pub struct SseFrameParser {
    /// Maximum size in bytes of one line, its terminator excluded. A
    /// longer line fails with [`SseFrameParserError::LineTooLarge`].
    /// Defaults to [`SSE_DEFAULT_MAX_LINE_LEN`].
    pub max_line_len: usize,
    /// Maximum size in bytes of [`SseFrame::data`]. An event growing
    /// past it fails with [`SseFrameParserError::EventTooLarge`].
    /// Defaults to [`SSE_DEFAULT_MAX_EVENT_LEN`].
    pub max_event_len: usize,
    buf: Vec<u8>,
    /// Where the bytes not consumed yet start in `buf`.
    start: usize,
    /// How many bytes past `start` hold no line end.
    scanned: usize,
    /// Whether the stream start was checked for a byte order mark.
    bom_checked: bool,
    event: Option<String>,
    data: String,
    last_event_id: Option<String>,
    retry: Option<u64>,
}

impl Default for SseFrameParser {
    fn default() -> Self {
        Self {
            max_line_len: SSE_DEFAULT_MAX_LINE_LEN,
            max_event_len: SSE_DEFAULT_MAX_EVENT_LEN,
            buf: Vec::new(),
            start: 0,
            scanned: 0,
            bom_checked: false,
            event: None,
            data: String::new(),
            last_event_id: None,
            retry: None,
        }
    }
}

impl SseFrameParser {
    /// Last-event-id seen so far; persists across dispatched frames so a
    /// reconnecting caller can resume via the `Last-Event-ID` header.
    pub fn last_event_id(&self) -> Option<&str> {
        self.last_event_id.as_deref()
    }

    /// Returns (line_end_excl_terminator, total_consumed) past `start`, or
    /// None when the buffer doesn't yet contain a complete line. Terminator
    /// may be \r\n, \n, or bare \r; a trailing \r is treated as incomplete
    /// pending \n.
    fn next_line(&mut self) -> Option<(usize, usize)> {
        let rest = &self.buf[self.start..];
        let from = self.scanned;

        let Some(eol) = position(&rest[from..], |b| (b == CR) | (b == LF)) else {
            self.scanned = rest.len();
            return None;
        };

        let eol = from + eol;

        let consumed = match (rest[eol], rest.get(eol + 1)) {
            (LF, _) => eol + 1,
            (_, None) => {
                self.scanned = eol;
                return None;
            }
            (_, Some(&LF)) => eol + 2,
            (_, Some(_)) => eol + 1,
        };

        self.scanned = 0;
        Some((eol, consumed))
    }
}

impl HttpCoroutine for SseFrameParser {
    type Yield = SseFrameParserYield;
    type Return = Result<Infallible, SseFrameParserError>;

    fn resume(&mut self, arg: Option<&[u8]>) -> HttpCoroutineState<Self::Yield, Self::Return> {
        if let Some(data) = arg {
            if self.start > 0 {
                self.buf.drain(..self.start);
                self.start = 0;
            }

            self.buf.extend_from_slice(data);
        }

        if !self.bom_checked {
            let head = &self.buf[self.start..];
            let n = head.len().min(BOM.len());

            if head[..n] != BOM[..n] {
                self.bom_checked = true;
            } else if n == BOM.len() {
                self.start += n;
                self.bom_checked = true;
            } else {
                return HttpCoroutineState::Yielded(SseFrameParserYield::WantsBytes);
            }
        }

        loop {
            let Some((len, consumed)) = self.next_line() else {
                if self.scanned > self.max_line_len {
                    let err = SseFrameParserError::LineTooLarge(self.max_line_len);
                    return HttpCoroutineState::Complete(Err(err));
                }

                return HttpCoroutineState::Yielded(SseFrameParserYield::WantsBytes);
            };

            if len > self.max_line_len {
                let err = SseFrameParserError::LineTooLarge(self.max_line_len);
                return HttpCoroutineState::Complete(Err(err));
            }

            let line = &self.buf[self.start..self.start + len];
            self.start += consumed;

            if line.is_empty() {
                if self.data.is_empty() && self.event.is_none() {
                    continue;
                }

                if self.data.ends_with('\n') {
                    self.data.pop();
                }

                let frame = SseFrame {
                    event: self.event.take(),
                    data: mem::take(&mut self.data),
                    id: self.last_event_id.clone(),
                    retry: self.retry.take(),
                };
                return HttpCoroutineState::Yielded(SseFrameParserYield::Frame(frame));
            }

            if line[0] == b':' {
                continue;
            }

            let (name, value) = split_field(line);
            let Ok(name) = str::from_utf8(name) else {
                trace!("ignore field with non-utf8 name");
                continue;
            };
            let Ok(value) = str::from_utf8(value) else {
                trace!("ignore field with non-utf8 value");
                continue;
            };

            match name {
                "event" => self.event = Some(value.to_string()),
                "data" => {
                    if self.data.len() + value.len() > self.max_event_len {
                        // NOTE: rewinds the line, a later resume fails again
                        self.start -= consumed;

                        let err = SseFrameParserError::EventTooLarge(self.max_event_len);
                        return HttpCoroutineState::Complete(Err(err));
                    }

                    self.data.push_str(value);
                    self.data.push('\n');
                }
                "id" => {
                    if !value.contains('\0') {
                        self.last_event_id = Some(value.to_string());
                    }
                }
                "retry" => {
                    if let Ok(n) = value.parse::<u64>() {
                        self.retry = Some(n);
                    }
                }
                _ => trace!("ignore unknown field `{name}`"),
            }
        }
    }
}

/// Splits a non-empty SSE line on the first `:`; a single leading SP
/// in the value is stripped per spec.
fn split_field(line: &[u8]) -> (&[u8], &[u8]) {
    match position(line, |b| b == b':') {
        None => (line, &[]),
        Some(colon) => {
            let name = &line[..colon];
            let mut value = &line[colon + 1..];
            if value.first() == Some(&b' ') {
                value = &value[1..];
            }
            (name, value)
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use crate::sse::frame::*;

    fn collect(stream: &[u8]) -> Vec<SseFrame> {
        collect_slices(&[stream])
    }

    fn collect_slices(slices: &[&[u8]]) -> Vec<SseFrame> {
        let mut parser = SseFrameParser::default();
        let mut frames = Vec::new();

        for slice in slices {
            let mut arg = Some(*slice);

            loop {
                match parser.resume(arg.take()) {
                    HttpCoroutineState::Yielded(SseFrameParserYield::Frame(frame)) => {
                        frames.push(frame)
                    }
                    HttpCoroutineState::Yielded(SseFrameParserYield::WantsBytes) => break,
                    HttpCoroutineState::Complete(Err(err)) => panic!("{err}"),
                }
            }
        }

        frames
    }

    /// The error a step completed with, `None` when it wants bytes.
    fn end(
        state: HttpCoroutineState<SseFrameParserYield, Result<Infallible, SseFrameParserError>>,
    ) -> Option<SseFrameParserError> {
        match state {
            HttpCoroutineState::Yielded(SseFrameParserYield::WantsBytes) => None,
            HttpCoroutineState::Yielded(SseFrameParserYield::Frame(frame)) => {
                panic!("got {frame:?}")
            }
            HttpCoroutineState::Complete(Err(err)) => Some(err),
        }
    }

    #[test]
    fn single_data_event() {
        let frames = collect(b"data: hello\n\n");
        assert_eq!(
            frames,
            vec![SseFrame {
                event: None,
                data: "hello".into(),
                id: None,
                retry: None,
            }]
        );
    }

    #[test]
    fn multi_line_data_joined_by_newline() {
        let frames = collect(b"data: hello\ndata: world\n\n");
        assert_eq!(frames[0].data, "hello\nworld");
    }

    #[test]
    fn event_and_id_fields() {
        let frames = collect(b"event: state\ndata: x\nid: 42\n\n");
        assert_eq!(frames[0].event.as_deref(), Some("state"));
        assert_eq!(frames[0].data, "x");
        assert_eq!(frames[0].id.as_deref(), Some("42"));
    }

    #[test]
    fn retry_parsed_when_integer() {
        let frames = collect(b"retry: 5000\ndata: x\n\n");
        assert_eq!(frames[0].retry, Some(5000));
    }

    #[test]
    fn retry_ignored_when_non_integer() {
        let frames = collect(b"retry: hello\ndata: x\n\n");
        assert_eq!(frames[0].retry, None);
    }

    #[test]
    fn comment_lines_ignored() {
        let frames = collect(b": keep-alive\ndata: x\n\n");
        assert_eq!(frames[0].data, "x");
    }

    #[test]
    fn empty_event_no_dispatch() {
        let frames = collect(b"\n\n\n");
        assert!(frames.is_empty());
    }

    #[test]
    fn id_persists_across_events() {
        let mut parser = SseFrameParser::default();
        let mut arg: Option<&[u8]> = Some(b"id: 1\ndata: a\n\ndata: b\n\n");
        let mut frames = Vec::new();

        loop {
            match parser.resume(arg.take()) {
                HttpCoroutineState::Yielded(SseFrameParserYield::Frame(frame)) => {
                    frames.push(frame)
                }
                HttpCoroutineState::Yielded(SseFrameParserYield::WantsBytes) => break,
                HttpCoroutineState::Complete(Err(err)) => panic!("{err}"),
            }
        }

        assert_eq!(frames[0].id.as_deref(), Some("1"));
        assert_eq!(frames[1].id.as_deref(), Some("1"));
        assert_eq!(parser.last_event_id(), Some("1"));
    }

    #[test]
    fn id_with_null_is_ignored() {
        let mut parser = SseFrameParser::default();
        let stream = b"id: bad\0\ndata: x\n\n";
        let arg: Option<&[u8]> = Some(stream);

        match parser.resume(arg) {
            HttpCoroutineState::Yielded(SseFrameParserYield::Frame(_)) => {}
            HttpCoroutineState::Yielded(SseFrameParserYield::WantsBytes) => {
                unreachable!("wants bytes");
            }
            HttpCoroutineState::Complete(Err(err)) => panic!("{err}"),
        }

        assert_eq!(parser.last_event_id(), None);
    }

    #[test]
    fn crlf_line_separator() {
        let frames = collect(b"data: hello\r\n\r\n");
        assert_eq!(frames[0].data, "hello");
    }

    #[test]
    fn bare_cr_line_separator() {
        let frames = collect(b"data: hello\r\rTAIL");
        assert_eq!(frames[0].data, "hello");
    }

    #[test]
    fn bom_stripped_at_stream_start() {
        let frames = collect(b"\xEF\xBB\xBFdata: hello\n\n");
        assert_eq!(frames[0].data, "hello");
    }

    #[test]
    fn bom_split_across_resumes_stripped() {
        let frames = collect_slices(&[b"\xEF\xBB", b"\xBFdata: hello\n\n"]);
        assert_eq!(frames[0].data, "hello");
    }

    #[test]
    fn bom_after_first_line_kept() {
        let frames = collect_slices(&[b"\n", b"\xEF\xBB\xBFdata: hello\n\n"]);
        assert!(frames.is_empty());
    }

    #[test]
    fn field_value_leading_space_stripped() {
        let frames = collect(b"data:  hello\n\n");
        assert_eq!(frames[0].data, " hello");
    }

    #[test]
    fn field_no_value() {
        let frames = collect(b"data\n\n");
        assert_eq!(frames[0].data, "");
    }

    #[test]
    fn incomplete_then_resumed() {
        let mut parser = SseFrameParser::default();
        let mut arg: Option<&[u8]> = Some(b"data: hel");
        let mut frames = Vec::new();

        loop {
            match parser.resume(arg.take()) {
                HttpCoroutineState::Yielded(SseFrameParserYield::Frame(frame)) => {
                    frames.push(frame);
                    break;
                }
                HttpCoroutineState::Yielded(SseFrameParserYield::WantsBytes) => {
                    if arg.is_none() {
                        arg = Some(b"lo\n\n");
                    } else {
                        break;
                    }
                }
                HttpCoroutineState::Complete(Err(err)) => panic!("{err}"),
            }
        }

        assert_eq!(frames[0].data, "hello");
    }

    #[test]
    fn byte_by_byte_matches_whole() {
        let stream = b"event: a\r\ndata: x\r\rdata: y\n: c\r\n\r\nid: 2\rdata: z\r\n\n";
        let bytes: Vec<&[u8]> = stream.chunks(1).collect();
        let frames = collect_slices(&bytes);

        assert_eq!(frames.len(), 3);
        assert_eq!(frames, collect(stream));
    }

    #[test]
    fn partial_line_over_default_limit_errors() {
        let mut parser = SseFrameParser::default();
        let line = vec![b'a'; SSE_DEFAULT_MAX_LINE_LEN + 1];
        let err = SseFrameParserError::LineTooLarge(SSE_DEFAULT_MAX_LINE_LEN);

        let state = parser.resume(Some(&line));
        assert_eq!(end(state), Some(err));
    }

    #[test]
    fn unfinished_event_over_default_limit_errors() {
        let mut parser = SseFrameParser::default();
        let lines = b"data: 0123456789abcdef\n".repeat(64 * 1024 + 1);
        let err = SseFrameParserError::EventTooLarge(SSE_DEFAULT_MAX_EVENT_LEN);

        let state = parser.resume(Some(&lines));
        assert_eq!(end(state), Some(err));
    }

    #[test]
    fn line_at_limit_passes() {
        let mut parser = SseFrameParser {
            max_line_len: 8,
            ..Default::default()
        };

        let state = parser.resume(Some(b"data: ab\r"));
        assert_eq!(end(state), None);

        let state = parser.resume(Some(b"\n\n"));
        let HttpCoroutineState::Yielded(SseFrameParserYield::Frame(frame)) = state else {
            panic!("got {state:?}");
        };
        assert_eq!(frame.data, "ab");
    }

    #[test]
    fn line_over_limit_errors_whole_or_split() {
        let stream = b"data: abc\n\n";
        let err = Some(SseFrameParserError::LineTooLarge(8));

        let mut parser = SseFrameParser {
            max_line_len: 8,
            ..Default::default()
        };
        assert_eq!(end(parser.resume(Some(stream))), err);

        let mut parser = SseFrameParser {
            max_line_len: 8,
            ..Default::default()
        };
        assert_eq!(end(parser.resume(Some(&stream[..8]))), None);
        assert_eq!(end(parser.resume(Some(&stream[8..9]))), err);
    }

    #[test]
    fn event_at_limit_passes() {
        let mut parser = SseFrameParser {
            max_event_len: 5,
            ..Default::default()
        };

        let state = parser.resume(Some(b"data: ab\ndata: cd\n\n"));
        let HttpCoroutineState::Yielded(SseFrameParserYield::Frame(frame)) = state else {
            panic!("got {state:?}");
        };
        assert_eq!(frame.data, "ab\ncd");
    }

    #[test]
    fn event_over_limit_errors_again_on_resume() {
        let mut parser = SseFrameParser {
            max_event_len: 5,
            ..Default::default()
        };

        let state = parser.resume(Some(b"data: ab\ndata: cde\n\n"));
        assert_eq!(end(state), Some(SseFrameParserError::EventTooLarge(5)));

        let state = parser.resume(Some(b"data: f\n\n"));
        assert_eq!(end(state), Some(SseFrameParserError::EventTooLarge(5)));
    }

    #[test]
    fn empty_data_lines_over_limit_errors() {
        let mut parser = SseFrameParser {
            max_event_len: 2,
            ..Default::default()
        };

        let state = parser.resume(Some(b"data\ndata\ndata\ndata\n"));
        assert_eq!(end(state), Some(SseFrameParserError::EventTooLarge(2)));
    }

    #[test]
    fn unknown_field_ignored() {
        let frames = collect(b"foobar: x\ndata: y\n\n");
        assert_eq!(frames[0].data, "y");
    }

    #[test]
    fn event_resets_after_dispatch() {
        let frames = collect(b"event: a\ndata: x\n\ndata: y\n\n");
        assert_eq!(frames[0].event.as_deref(), Some("a"));
        assert_eq!(frames[1].event, None);
    }
}
