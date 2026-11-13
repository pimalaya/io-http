#![no_main]

//! Coverage-guided fuzz target for the response parsers. The oracle:
//! whatever a server sends, the HTTP/1.0 and HTTP/1.1 send coroutines,
//! the streaming chunk decoder and the event stream parser answer or
//! fail, never panic, never spin on EOF, never return more than the
//! limits they were given, and return the same thing whether the bytes
//! come split or whole.

use std::fmt::Display;

use arbitrary::Arbitrary;
use io_http::{
    coroutine::*,
    rfc1945::send::Http10Send,
    rfc9110::{request::HttpRequest, send::*},
    rfc9112::{
        chunk_stream::{Http11ChunksReadStream, Http11ChunksReadStreamYield},
        send::Http11Send,
    },
    sse::frame::*,
};
use libfuzzer_sys::fuzz_target;
use url::Url;

/// How many reads a coroutine may still ask for once EOF was reported.
const MAX_READS_AFTER_EOF: usize = 2;

/// An exchange as its caller sees it, the bytes left past it excluded.
#[derive(Debug, PartialEq)]
enum Exchange {
    Response {
        redirect: Option<Url>,
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
        keep_alive: bool,
    },
    Err(String),
}

#[derive(Arbitrary, Debug)]
struct Input {
    max_headers_len: u16,
    max_body_len: u16,
    reads: Vec<Vec<u8>>,
}

fuzz_target!(|input: Input| {
    let max_headers_len = usize::from(input.max_headers_len);
    let max_body_len = usize::from(input.max_body_len);

    let whole = input.reads.concat();
    let whole: Vec<&[u8]> = [whole.as_slice()].into_iter().collect();
    let split: Vec<&[u8]> = input.reads.iter().map(Vec::as_slice).collect();

    let url = Url::parse("http://example.com/").unwrap();
    let request = HttpRequest::get(url).header("Host", "example.com");

    let exchange = |reads: &[&[u8]]| {
        let mut send = Http11Send::new(request.clone());
        send.max_headers_len = max_headers_len;
        send.max_body_len = max_body_len;
        check_send(send, reads, max_body_len)
    };

    assert_eq!(exchange(&split), exchange(&whole));

    let exchange = |reads: &[&[u8]]| {
        let mut send = Http10Send::new(request.clone());
        send.max_headers_len = max_headers_len;
        send.max_body_len = max_body_len;
        check_send(send, reads, max_body_len)
    };

    assert_eq!(exchange(&split), exchange(&whole));

    let chunks = |reads: &[&[u8]]| check_chunk_stream(reads, max_body_len);
    assert_eq!(chunks(&split), chunks(&whole));

    let events = |reads: &[&[u8]]| parse_events(reads, max_headers_len, max_body_len);
    assert_eq!(events(&split), events(&whole));
});

fn check_send<C, E>(mut send: C, reads: &[&[u8]], max_body_len: usize) -> Exchange
where
    C: HttpCoroutine<Yield = HttpSendYield, Return = Result<HttpSendOutput, E>>,
    E: Display,
{
    let mut reads = reads.iter().filter(|r| !r.is_empty());
    let mut reads_after_eof = 0;
    let mut arg: Option<&[u8]> = None;

    let (redirect, response, keep_alive) = loop {
        match send.resume(arg.take()) {
            HttpCoroutineState::Yielded(HttpSendYield::WantsWrite(_)) => {}
            HttpCoroutineState::Yielded(HttpSendYield::WantsRead) => match reads.next() {
                Some(read) => arg = Some(read),
                None => {
                    reads_after_eof += 1;
                    assert!(reads_after_eof <= MAX_READS_AFTER_EOF, "spins on EOF");
                    arg = Some(b"");
                }
            },
            HttpCoroutineState::Yielded(HttpSendYield::WantsRedirect {
                url,
                response,
                keep_alive,
                ..
            }) => break (Some(url), response, keep_alive),
            HttpCoroutineState::Complete(Ok(out)) => break (None, out.response, out.keep_alive),
            HttpCoroutineState::Complete(Err(err)) => return Exchange::Err(err.to_string()),
        }
    };

    assert!(response.body.len() <= max_body_len);

    Exchange::Response {
        redirect,
        status: *response.status,
        headers: response.headers,
        body: response.body,
        keep_alive,
    }
}

fn check_chunk_stream(reads: &[&[u8]], max_len: usize) -> (Vec<Vec<u8>>, Option<String>) {
    let mut chunks = Http11ChunksReadStream::default();
    chunks.max_chunk_len = max_len;

    let mut reads = reads.iter().filter(|r| !r.is_empty());
    let mut reads_after_eof = 0;
    let mut arg: Option<&[u8]> = None;
    let mut frames = Vec::new();

    loop {
        match chunks.resume(arg.take()) {
            HttpCoroutineState::Yielded(Http11ChunksReadStreamYield::Frame { body }) => {
                assert!(body.len() <= max_len);
                frames.push(body);
            }
            HttpCoroutineState::Yielded(Http11ChunksReadStreamYield::WantsRead) => {
                match reads.next() {
                    Some(read) => arg = Some(read),
                    None => {
                        reads_after_eof += 1;
                        assert!(reads_after_eof <= MAX_READS_AFTER_EOF, "spins on EOF");
                        arg = Some(b"");
                    }
                }
            }
            HttpCoroutineState::Complete(Ok(_)) => return (frames, None),
            HttpCoroutineState::Complete(Err(err)) => return (frames, Some(err.to_string())),
        }
    }
}

fn parse_events(
    reads: &[&[u8]],
    max_line_len: usize,
    max_event_len: usize,
) -> (Vec<SseFrame>, Option<SseFrameParserError>) {
    let mut parser = SseFrameParser::default();
    parser.max_line_len = max_line_len;
    parser.max_event_len = max_event_len;

    let mut frames = Vec::new();

    for read in reads {
        let mut arg = Some(*read);

        loop {
            match parser.resume(arg.take()) {
                HttpCoroutineState::Yielded(SseFrameParserYield::Frame(frame)) => {
                    assert!(frame.data.len() <= max_event_len);
                    assert!(frame.event.as_ref().is_none_or(|e| e.len() <= max_line_len));
                    assert!(frame.id.as_ref().is_none_or(|id| id.len() <= max_line_len));
                    frames.push(frame);
                }
                HttpCoroutineState::Yielded(SseFrameParserYield::WantsBytes) => break,
                HttpCoroutineState::Complete(Err(err)) => return (frames, Some(err)),
            }
        }
    }

    (frames, None)
}
